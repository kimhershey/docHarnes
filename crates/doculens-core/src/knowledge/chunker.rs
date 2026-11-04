//! Structure-aware chunking (design §3.14.1): page text → blocks (headings,
//! paragraphs, table rows, list items) → chunks of ~400–800 tokens that never
//! cross a 장/절/조 boundary, each carrying its heading path ("제3장 복무 >
//! 제12조 출장") and clause label ("제12조 제2항") plus the page range.
//!
//! The input is the text layer stored per page (digital PDFs, converted
//! HWPX/DOCX/HWP); OCR text (P4) feeds the same function.

use super::tokenize::{class_of, CharClass};
use super::{ChunkDraft, ChunkKind};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChunkOptions {
    pub target_tokens: u32,
    pub max_tokens: u32,
    pub min_tokens: u32,
}

impl Default for ChunkOptions {
    fn default() -> Self {
        ChunkOptions { target_tokens: 500, max_tokens: 800, min_tokens: 60 }
    }
}

impl ChunkOptions {
    pub fn with_target(target: u32) -> ChunkOptions {
        let target = target.clamp(120, 2_000);
        ChunkOptions {
            target_tokens: target,
            max_tokens: (target as f32 * 1.6) as u32,
            min_tokens: (target / 8).max(30),
        }
    }
}

pub struct PageText<'a> {
    pub page_no: u32,
    pub text: &'a str,
}

/// Rough token count for Korean/English mixed text (conservative: Qwen/bge
/// tokenizers need ~0.6–0.8 tokens per Hangul syllable).
pub fn estimate_tokens(text: &str) -> u32 {
    let (mut hangul, mut cjk, mut latin_words, mut digits, mut other) = (0f32, 0f32, 0f32, 0f32, 0f32);
    let mut in_latin = false;
    for c in text.chars() {
        match class_of(c) {
            CharClass::Hangul => {
                hangul += 1.0;
                in_latin = false;
            }
            CharClass::Cjk => {
                cjk += 1.0;
                in_latin = false;
            }
            CharClass::Digit => {
                digits += 1.0;
                in_latin = false;
            }
            CharClass::Latin => {
                if !in_latin {
                    latin_words += 1.0;
                    in_latin = true;
                }
            }
            CharClass::Other => {
                in_latin = false;
                if !c.is_whitespace() {
                    other += 1.0;
                }
            }
        }
    }
    (hangul * 0.75 + cjk + latin_words * 1.3 + digits * 0.5 + other * 0.5).ceil() as u32
}

#[derive(Debug, Clone, PartialEq)]
struct Heading {
    /// 1 장/부칙/I., 2 절/N./[제목], 3 조/N.N, 4 항 ①/(1), 5 호 1)/N., 6 목 가.
    level: u8,
    title: String,
    /// Label used in `clause` ("제12조", "제2항", "제1호", "가목", "2.1").
    label: Option<String>,
    /// Body text that followed the heading on the same line ("제4조(정산) 출장 종료 후 …").
    body: Option<String>,
}

#[derive(Debug, Clone)]
struct Block {
    page: u32,
    kind: ChunkKind,
    text: String,
    tokens: u32,
    heading_path: Option<String>,
    clause: Option<String>,
    /// Heading level when this block is a heading line (section headings start chunks).
    heading_level: Option<u8>,
}

fn is_digit_run(chars: &[char]) -> bool {
    !chars.is_empty() && chars.len() <= 4 && chars.iter().all(|c| c.is_ascii_digit())
}

fn circled_number(c: char) -> Option<u32> {
    let u = c as u32;
    if (0x2460..=0x2473).contains(&u) {
        Some(u - 0x2460 + 1)
    } else if (0x2474..=0x2487).contains(&u) {
        Some(u - 0x2474 + 1)
    } else if (0x2488..=0x249B).contains(&u) {
        Some(u - 0x2488 + 1)
    } else {
        None
    }
}

const HANGUL_ITEMS: &[char] = &['가', '나', '다', '라', '마', '바', '사', '아', '자', '차', '카', '타', '파', '하'];

/// Recognises a heading / item line. `None` for ordinary text.
fn detect_heading(line: &str) -> Option<Heading> {
    let t = line.trim();
    if t.is_empty() || t.chars().count() > 90 {
        return None;
    }
    let chars: Vec<char> = t.chars().collect();
    // Markdown
    if chars[0] == '#' {
        let n = chars.iter().take_while(|c| **c == '#').count();
        if n <= 6 && chars.get(n) == Some(&' ') {
            let title: String = chars[n..].iter().collect::<String>().trim().to_string();
            return Some(Heading { level: n.min(3) as u8, title, label: None, body: None });
        }
    }
    // 부칙
    if t == "부칙" || t.starts_with("부칙 ") || t.starts_with("부 칙") {
        return Some(Heading { level: 1, title: t.to_string(), label: Some("부칙".into()), body: None });
    }
    // 제N장 / 제N절 / 제N조 / 제N관 / 제N편
    if chars[0] == '제' {
        let mut i = 1;
        while i < chars.len() && chars[i] == ' ' {
            i += 1;
        }
        let ds = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i > ds && i - ds <= 4 {
            let mut k = i;
            while k < chars.len() && chars[k] == ' ' {
                k += 1;
            }
            if let Some(unit) = chars.get(k) {
                let level = match unit {
                    '편' => Some(1u8),
                    '장' => Some(1),
                    '절' | '관' => Some(2),
                    '조' => Some(3),
                    _ => None,
                };
                if let Some(level) = level {
                    // "제12조의2" keeps its suffix in the label.
                    let mut e = k + 1;
                    if chars.get(e) == Some(&'의') {
                        e += 1;
                        while e < chars.len() && chars[e].is_ascii_digit() {
                            e += 1;
                        }
                    }
                    let num: String = chars[ds..i].iter().collect();
                    let suffix: String = chars[k + 1..e].iter().collect();
                    let rest: String = chars[e..].iter().collect::<String>().trim().to_string();
                    // A 조 line that continues into a long sentence is body text ("제3조에 따라 …").
                    if level == 3
                        && (rest.starts_with('에')
                            || rest.starts_with("의 ")
                            || rest.starts_with('를')
                            || rest.starts_with('는'))
                    {
                        return None;
                    }
                    if level < 3 && rest.chars().count() > 60 {
                        return None;
                    }
                    let label = format!("제{num}{unit}{suffix}");
                    // "제4조(정산) 출장 종료 후 …" → title "제4조 정산", body "출장 종료 후 …";
                    // "제5조 출장 종료 후 … 한다." → title "제5조", body = rest.
                    let (title_rest, body) = if rest.starts_with('(') {
                        match rest.find(')') {
                            Some(close) => {
                                let inner = rest[1..close].trim().to_string();
                                let after = rest[close + 1..].trim().to_string();
                                (inner, if after.is_empty() { None } else { Some(after) })
                            }
                            None => (rest.clone(), None),
                        }
                    } else if rest.chars().count() > 30 || rest.ends_with('.') || rest.ends_with('다') {
                        (String::new(), Some(rest.clone()))
                    } else {
                        (rest.clone(), None)
                    };
                    let title = if title_rest.is_empty() { label.clone() } else { format!("{label} {title_rest}") };
                    return Some(Heading { level, title, label: Some(label), body });
                }
            }
        }
    }
    // ① / (1) / 1) → 항 or 호
    if let Some(n) = circled_number(chars[0]) {
        return Some(Heading { level: 4, title: t.to_string(), label: Some(format!("제{n}항")), body: None });
    }
    if chars[0] == '(' {
        let close = chars.iter().position(|c| *c == ')');
        if let Some(close) = close {
            let inner = &chars[1..close];
            if is_digit_run(inner) {
                let n: String = inner.iter().collect();
                return Some(Heading { level: 4, title: t.to_string(), label: Some(format!("제{n}항")), body: None });
            }
            if inner.len() == 1 && HANGUL_ITEMS.contains(&inner[0]) {
                return Some(Heading {
                    level: 6,
                    title: t.to_string(),
                    label: Some(format!("{}목", inner[0])),
                    body: None,
                });
            }
        }
    }
    // 1) / 1. / 1.2 / 1.2.3 / 가.
    let mut i = 0;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i <= 3 {
        if chars.get(i) == Some(&')') {
            let n: String = chars[..i].iter().collect();
            return Some(Heading { level: 5, title: t.to_string(), label: Some(format!("제{n}호")), body: None });
        }
        if chars.get(i) == Some(&'.') {
            // dotted sections
            let mut depth = 1;
            let mut j = i + 1;
            let mut num_end = i;
            loop {
                let ds = j;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if j > ds {
                    depth += 1;
                    num_end = j;
                    if chars.get(j) == Some(&'.') {
                        j += 1;
                        continue;
                    }
                }
                break;
            }
            let label: String = chars[..num_end].iter().collect::<String>().trim_end_matches('.').to_string();
            let rest: String = chars[num_end..].iter().collect::<String>().trim_start_matches('.').trim().to_string();
            if depth >= 2 {
                if rest.is_empty() {
                    return None;
                }
                return Some(Heading {
                    level: (depth as u8 + 1).min(3),
                    title: t.to_string(),
                    label: Some(label),
                    body: None,
                });
            }
            // "1. 제목" (short, no sentence ending) is a section; "1. 긴 문장…다." is a list item (호).
            let short =
                rest.chars().count() <= 40 && !rest.ends_with('.') && !rest.ends_with('다') && !rest.contains(": ");
            let level = if short { 2 } else { 5 };
            let label = if short { Some(label) } else { Some(format!("제{label}호")) };
            return Some(Heading { level, title: t.to_string(), label, body: None });
        }
    }
    if chars.len() >= 2 && HANGUL_ITEMS.contains(&chars[0]) && chars[1] == '.' {
        return Some(Heading { level: 6, title: t.to_string(), label: Some(format!("{}목", chars[0])), body: None });
    }
    // Roman numerals (I. II. Ⅰ.)
    let roman_end = chars
        .iter()
        .take_while(|c| matches!(c, 'I' | 'V' | 'X' | 'Ⅰ' | 'Ⅱ' | 'Ⅲ' | 'Ⅳ' | 'Ⅴ' | 'Ⅵ' | 'Ⅶ' | 'Ⅷ' | 'Ⅸ' | 'Ⅹ'))
        .count();
    if roman_end > 0 && roman_end <= 4 && chars.get(roman_end) == Some(&'.') && chars.len() > roman_end + 1 {
        let label: String = chars[..roman_end].iter().collect();
        return Some(Heading { level: 1, title: t.to_string(), label: Some(label), body: None });
    }
    // [제목] / 【제목】 / <제목>
    if (chars[0] == '[' && chars[chars.len() - 1] == ']' || chars[0] == '【' && chars[chars.len() - 1] == '】')
        && chars.len() <= 42
    {
        return Some(Heading { level: 2, title: t.to_string(), label: None, body: None });
    }
    None
}

fn is_table_line(line: &str) -> bool {
    let tabs = line.matches('\t').count();
    let pipes = line.matches('|').count();
    if tabs >= 2 || pipes >= 2 {
        return true;
    }
    // ≥ 3 cells separated by runs of 2+ spaces
    let cells = line.split("  ").filter(|c| !c.trim().is_empty()).count();
    cells >= 3
}

fn is_sentence_end(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '。' | '！' | '？')
}

/// Splits an oversize paragraph at sentence boundaries into pieces ≤ `max_tokens`.
fn split_long_text(text: &str, max_tokens: u32) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut current_tokens = 0u32;
    let mut sentence = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        sentence.push(chars[i]);
        let boundary =
            chars[i] == '\n' || (is_sentence_end(chars[i]) && chars.get(i + 1).is_none_or(|n| n.is_whitespace()));
        if boundary || i + 1 == chars.len() {
            let st = estimate_tokens(&sentence);
            if st > max_tokens {
                // A single enormous sentence: cut by characters.
                let mut buf = String::new();
                for c in sentence.chars() {
                    buf.push(c);
                    if estimate_tokens(&buf) >= max_tokens {
                        pieces.push(std::mem::take(&mut buf));
                    }
                }
                if !buf.trim().is_empty() {
                    pieces.push(buf);
                }
                sentence.clear();
            } else {
                if current_tokens + st > max_tokens && !current.trim().is_empty() {
                    pieces.push(std::mem::take(&mut current).trim().to_string());
                    current_tokens = 0;
                }
                current.push_str(&sentence);
                current_tokens += st;
                sentence.clear();
            }
        }
        i += 1;
    }
    if !current.trim().is_empty() {
        pieces.push(current.trim().to_string());
    }
    pieces.into_iter().filter(|p| !p.trim().is_empty()).collect()
}

struct HeadingStack {
    items: Vec<Heading>,
}

impl HeadingStack {
    fn push(&mut self, h: Heading) {
        self.items.retain(|x| x.level < h.level);
        self.items.push(h);
    }
    fn path(&self) -> Option<String> {
        let parts: Vec<&str> = self.items.iter().filter(|h| h.level <= 3).map(|h| h.title.as_str()).collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" > "))
        }
    }
    fn clause(&self) -> Option<String> {
        let parts: Vec<&str> = self.items.iter().filter(|h| h.level >= 3).filter_map(|h| h.label.as_deref()).collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
    }
}

fn blocks_for_pages(pages: &[PageText<'_>], opts: &ChunkOptions) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut stack = HeadingStack { items: Vec::new() };
    for page in pages {
        let mut para: Vec<String> = Vec::new();
        let mut table: Vec<String> = Vec::new();
        let flush_para = |para: &mut Vec<String>, blocks: &mut Vec<Block>, stack: &HeadingStack, kind: ChunkKind| {
            if para.is_empty() {
                return;
            }
            let text = para.join("\n");
            para.clear();
            for piece in split_long_text(&text, opts.max_tokens) {
                let tokens = estimate_tokens(&piece);
                blocks.push(Block {
                    page: page.page_no,
                    kind,
                    text: piece,
                    tokens,
                    heading_path: stack.path(),
                    clause: stack.clause(),
                    heading_level: None,
                });
            }
        };
        let flush_table = |table: &mut Vec<String>, blocks: &mut Vec<Block>, stack: &HeadingStack| {
            if table.is_empty() {
                return;
            }
            let header = table[0].clone();
            let mut group: Vec<String> = Vec::new();
            let mut group_tokens = 0u32;
            let rows = std::mem::take(table);
            let mut first = true;
            for row in rows {
                let rt = estimate_tokens(&row);
                if group_tokens + rt > opts.max_tokens && !group.is_empty() {
                    let text = group.join("\n");
                    blocks.push(Block {
                        page: page.page_no,
                        kind: ChunkKind::Table,
                        tokens: estimate_tokens(&text),
                        text,
                        heading_path: stack.path(),
                        clause: stack.clause(),
                        heading_level: None,
                    });
                    group.clear();
                    group_tokens = 0;
                    if !first {
                        group.push(header.clone());
                        group_tokens = estimate_tokens(&header);
                    }
                }
                group_tokens += rt;
                group.push(row);
                first = false;
            }
            if !group.is_empty() {
                let text = group.join("\n");
                blocks.push(Block {
                    page: page.page_no,
                    kind: ChunkKind::Table,
                    tokens: estimate_tokens(&text),
                    text,
                    heading_path: stack.path(),
                    clause: stack.clause(),
                    heading_level: None,
                });
            }
        };
        for raw in page.text.lines() {
            let line = raw.trim_end();
            if line.trim().is_empty() {
                flush_para(&mut para, &mut blocks, &stack, ChunkKind::Text);
                flush_table(&mut table, &mut blocks, &stack);
                continue;
            }
            if let Some(h) = detect_heading(line) {
                flush_para(&mut para, &mut blocks, &stack, ChunkKind::Text);
                flush_table(&mut table, &mut blocks, &stack);
                if h.level <= 3 {
                    let body = h.body.clone();
                    stack.push(h.clone());
                    blocks.push(Block {
                        page: page.page_no,
                        kind: ChunkKind::Heading,
                        text: h.title.clone(),
                        tokens: estimate_tokens(&h.title),
                        heading_path: stack.path(),
                        clause: stack.clause(),
                        heading_level: Some(h.level),
                    });
                    if let Some(body) = body {
                        para.push(body);
                    }
                } else {
                    // 호/목 items stay inline (list block) but refine the clause label.
                    stack.push(h.clone());
                    para.push(line.trim().to_string());
                    flush_para(&mut para, &mut blocks, &stack, ChunkKind::List);
                }
                continue;
            }
            if is_table_line(line) {
                flush_para(&mut para, &mut blocks, &stack, ChunkKind::Text);
                table.push(line.trim().to_string());
                continue;
            }
            flush_table(&mut table, &mut blocks, &stack);
            para.push(line.trim().to_string());
        }
        flush_para(&mut para, &mut blocks, &stack, ChunkKind::Text);
        flush_table(&mut table, &mut blocks, &stack);
    }
    blocks
}

struct Packer<'o> {
    opts: &'o ChunkOptions,
    out: Vec<ChunkDraft>,
    current: Vec<Block>,
    tokens: u32,
}

impl Packer<'_> {
    fn flush(&mut self) {
        if self.current.is_empty() {
            return;
        }
        let blocks = std::mem::take(&mut self.current);
        self.tokens = 0;
        let content: Vec<&Block> = blocks.iter().filter(|b| b.heading_level.is_none()).collect();
        // Label the chunk by the deepest section heading it contains (first
        // occurrence), else by the context its first content block inherited.
        let deepest = blocks.iter().filter_map(|b| b.heading_level).max();
        let meta_source = deepest
            .and_then(|lvl| blocks.iter().find(|b| b.heading_level == Some(lvl)))
            .or_else(|| content.first().copied())
            .or(blocks.last());
        let kind = if content.is_empty() {
            ChunkKind::Heading
        } else if content.iter().all(|b| b.kind == ChunkKind::Table) {
            ChunkKind::Table
        } else if content.iter().all(|b| b.kind == ChunkKind::List) {
            ChunkKind::List
        } else {
            ChunkKind::Text
        };
        let text = blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
        let draft = ChunkDraft {
            page_start: blocks.iter().map(|b| b.page).min().unwrap_or(1),
            page_end: blocks.iter().map(|b| b.page).max().unwrap_or(1),
            kind,
            heading_path: meta_source.and_then(|b| b.heading_path.clone()),
            clause: meta_source.and_then(|b| b.clause.clone()),
            token_estimate: estimate_tokens(&text),
            text,
        };
        // A tiny trailing fragment under the same heading joins the previous chunk.
        if draft.token_estimate < self.opts.min_tokens {
            if let Some(prev) = self.out.last_mut() {
                if prev.heading_path == draft.heading_path
                    && prev.token_estimate + draft.token_estimate <= self.opts.max_tokens
                {
                    prev.text.push('\n');
                    prev.text.push_str(&draft.text);
                    prev.page_end = prev.page_end.max(draft.page_end);
                    prev.token_estimate = estimate_tokens(&prev.text);
                    if prev.kind != draft.kind {
                        prev.kind = ChunkKind::Text;
                    }
                    return;
                }
            }
        }
        self.out.push(draft);
    }

    fn push(&mut self, block: Block) {
        let section_start = block.heading_level.is_some_and(|l| l <= 3);
        if section_start && self.tokens >= self.opts.min_tokens {
            self.flush();
        }
        let over_max = self.tokens + block.tokens > self.opts.max_tokens && !self.current.is_empty();
        let over_target = self.tokens + block.tokens > self.opts.target_tokens
            && self.tokens >= self.opts.min_tokens
            && block.heading_level.is_none();
        if over_max || over_target {
            self.flush();
        }
        self.tokens += block.tokens;
        self.current.push(block);
    }
}

/// Chunks the page texts of one document in reading order.
pub fn chunk_document(pages: &[PageText<'_>], opts: &ChunkOptions) -> Vec<ChunkDraft> {
    let blocks = blocks_for_pages(pages, opts);
    let mut packer = Packer { opts, out: Vec::new(), current: Vec::new(), tokens: 0 };
    for b in blocks {
        packer.push(b);
    }
    packer.flush();
    packer.out.into_iter().filter(|c| !c.text.trim().is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGULATION: &str = "출장비 규정\n\n제1장 총칙\n제1조(목적) 이 규정은 임직원의 국내외 출장에 관한 사항을 정함을 목적으로 한다.\n제2조(적용 범위) 이 규정은 모든 임직원에게 적용한다.\n\n제2장 출장비\n제3조(출장비의 구성)\n① 출장비는 교통비, 숙박비, 일비로 구성한다.\n② 숙박비는 실비로 지급하되 1박당 150,000원을 한도로 한다.\n1. 국내 출장: 영수증 첨부\n2. 해외 출장: 환율은 출발일 기준\n제4조(정산) 출장 종료 후 7일 이내에 정산서를 제출하여야 한다.\n";

    #[test]
    fn regulation_chunks_carry_heading_paths_and_clauses() {
        let pages = [PageText { page_no: 1, text: REGULATION }];
        let chunks = chunk_document(&pages, &ChunkOptions { target_tokens: 60, max_tokens: 120, min_tokens: 10 });
        assert!(chunks.len() >= 3, "{chunks:#?}");
        let c3 = chunks.iter().find(|c| c.text.contains("숙박비는 실비로")).expect("제3조 chunk");
        assert_eq!(c3.heading_path.as_deref(), Some("제2장 출장비 > 제3조 출장비의 구성"));
        assert!(c3.clause.as_deref().is_some_and(|c| c.starts_with("제3조")), "{:?}", c3.clause);
        let item = chunks.iter().find(|c| c.text.contains("해외 출장")).unwrap();
        assert!(
            item.clause.as_deref().unwrap().contains("제2항") || item.clause.as_deref().unwrap().contains("제3조"),
            "{:?}",
            item.clause
        );
        let c4 = chunks.iter().find(|c| c.text.contains("정산서를 제출")).unwrap();
        assert_eq!(c4.clause.as_deref(), Some("제4조"));
        assert_eq!(c4.heading_path.as_deref(), Some("제2장 출장비 > 제4조 정산"));
        assert!(c4.text.contains("제4조 정산\n출장 종료 후"), "{}", c4.text);
        for c in &chunks {
            assert!(c.token_estimate <= 120, "chunk too large: {}", c.token_estimate);
            assert_eq!(c.page_start, 1);
        }
    }

    #[test]
    fn table_rows_group_into_table_chunks_with_repeated_header() {
        let mut text = String::from("품목 내역\n품목\t수량\t단가\t금액\n");
        for i in 0..60 {
            text.push_str(&format!("노트북 {i}\t{}\t1,200,000\t{}\n", i + 1, (i + 1) * 1_200_000));
        }
        let pages = [PageText { page_no: 2, text: &text }];
        let chunks = chunk_document(&pages, &ChunkOptions { target_tokens: 150, max_tokens: 200, min_tokens: 10 });
        let tables: Vec<&ChunkDraft> = chunks.iter().filter(|c| c.kind == ChunkKind::Table).collect();
        assert!(tables.len() >= 2, "{}", chunks.len());
        assert!(
            tables[1].text.starts_with("품목\t수량"),
            "header repeated: {}",
            tables[1].text.lines().next().unwrap()
        );
        assert!(tables.iter().all(|t| t.page_start == 2 && t.page_end == 2));
    }

    #[test]
    fn long_paragraph_is_split_at_sentences_and_pages_are_tracked() {
        let sentence = "이 문장은 길이를 늘리기 위한 예시 문장입니다. ";
        let p1 = sentence.repeat(80);
        let p2 = format!("제1조(목적) 다음 쪽 내용.\n{}", sentence.repeat(10));
        let pages = [PageText { page_no: 1, text: &p1 }, PageText { page_no: 2, text: &p2 }];
        let chunks = chunk_document(&pages, &ChunkOptions::default());
        assert!(chunks.len() >= 2);
        assert!(chunks.iter().all(|c| c.token_estimate <= 800));
        assert!(chunks.iter().any(|c| c.page_start == 2));
        assert!(chunks.iter().all(|c| !c.text.trim().is_empty()));
        // Sentences are not cut mid-way.
        for c in chunks.iter().filter(|c| c.page_start == 1) {
            assert!(c.text.trim_end().ends_with("입니다."), "{}", &c.text[c.text.len().saturating_sub(40)..]);
        }
    }

    #[test]
    fn empty_and_whitespace_pages_yield_nothing() {
        let pages = [PageText { page_no: 1, text: "   \n\n\t" }, PageText { page_no: 2, text: "" }];
        assert!(chunk_document(&pages, &ChunkOptions::default()).is_empty());
    }

    #[test]
    fn heading_detection_rules() {
        assert_eq!(detect_heading("제3조(출장비)").unwrap().level, 3);
        assert_eq!(detect_heading("제 12 조의2 (특례)").unwrap().label.as_deref(), Some("제12조의2"));
        let h = detect_heading("제4조(정산) 출장 종료 후 7일 이내에 정산서를 제출하여야 한다.").unwrap();
        assert_eq!(
            (h.title.as_str(), h.body.as_deref()),
            ("제4조 정산", Some("출장 종료 후 7일 이내에 정산서를 제출하여야 한다."))
        );
        let h = detect_heading("제5조 출장 종료 후 7일 이내에 정산한다.").unwrap();
        assert_eq!((h.title.as_str(), h.body.is_some()), ("제5조", true));
        assert!(detect_heading("제3조에 따라 지급한다.").is_none());
        assert_eq!(detect_heading("① 출장비는").unwrap().label.as_deref(), Some("제1항"));
        assert_eq!(detect_heading("(2) 숙박비").unwrap().label.as_deref(), Some("제2항"));
        assert_eq!(detect_heading("2.1 범위").unwrap().level, 3);
        assert_eq!(detect_heading("1. 개요").unwrap().level, 2);
        assert_eq!(detect_heading("1. 국내 출장은 영수증을 첨부하여 정산한다.").unwrap().level, 5);
        assert_eq!(detect_heading("가. 교통비").unwrap().label.as_deref(), Some("가목"));
        assert_eq!(detect_heading("## 설치").unwrap().level, 2);
        assert_eq!(detect_heading("[일반 조건]").unwrap().level, 2);
        assert!(detect_heading("이 규정은 임직원에게 적용한다.").is_none());
        assert!(detect_heading("2024.11.04").is_none());
    }

    #[test]
    fn token_estimate_is_monotonic_and_reasonable() {
        assert_eq!(estimate_tokens(""), 0);
        let ko = estimate_tokens("출장비 규정에 따라 정산한다");
        assert!((8..=14).contains(&ko), "{ko}");
        let en = estimate_tokens("The quick brown fox");
        assert!((4..=7).contains(&en), "{en}");
    }
}
