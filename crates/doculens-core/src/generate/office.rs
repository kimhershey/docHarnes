//! Minimal from-scratch DOCX (WordprocessingML) and HWPX (OWPML) writers for
//! a `DocModel`: paragraphs (title / headings / lists / alignment, bold /
//! italic / underline / size runs, line breaks) and tables (header rows,
//! column spans, borders). The structure mirrors what the readers in
//! `documents::convert::{docx, hwpx}` understand, so a written file reads
//! back into the same model (round-trip tests). HWP 5.0 binary output is
//! out of scope (§3.13: save HWPX, convert in Hancom Office).

use std::fmt::Write as _;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use zip::write::SimpleFileOptions;

use crate::documents::convert::html::escape;
use crate::documents::convert::model::{Align, Block, Cell, DocModel, ListKind, Paragraph, Run, Table};
use crate::error::{CoreError, Result};

/// A4 text width (11906 − 2 × 1440 twips of margins).
const DOCX_TEXT_TWIPS: u32 = 9_026;
/// A4 text width in HWPUNIT (59528 − 2 × 8504).
const HWPX_TEXT_UNITS: u32 = 42_520;
const HWPX_ROW_HEIGHT: u32 = 2_200;

fn zip_write(
    z: &mut zip::ZipWriter<File>,
    path: &Path,
    name: &str,
    data: &[u8],
    opts: SimpleFileOptions,
) -> Result<()> {
    z.start_file(name, opts).map_err(|e| CoreError::Convert(format!("{name}: {e}")))?;
    z.write_all(data).map_err(|e| CoreError::io(path, e))
}

fn col_count(t: &Table) -> usize {
    t.rows.iter().map(|r| r.iter().map(|c| c.col_span.max(1) as usize).sum::<usize>()).max().unwrap_or(1).max(1)
}

/// Column widths as shares (sum 1.0) from the model or equal.
fn col_shares(t: &Table, cols: usize) -> Vec<f32> {
    let widths: Vec<f32> = t.col_widths_mm.iter().copied().filter(|w| *w > 0.0).collect();
    if widths.len() == cols {
        let sum: f32 = widths.iter().sum();
        return widths.iter().map(|w| w / sum).collect();
    }
    vec![1.0 / cols as f32; cols]
}

fn cell_paragraphs(c: &Cell) -> Vec<Paragraph> {
    let mut out = Vec::new();
    for b in &c.blocks {
        match b {
            Block::Paragraph(p) => out.push(p.clone()),
            Block::Table(t) => {
                // Nested tables are flattened to tab-separated lines (not expected from the planner).
                for row in &t.rows {
                    let text: Vec<String> = row.iter().map(|c| c.text().trim().to_string()).collect();
                    out.push(Paragraph::plain(text.join("\t")));
                }
            }
            Block::Image { .. } | Block::PageBreak => {}
        }
    }
    if out.is_empty() {
        out.push(Paragraph::default());
    }
    out
}

// ---------------------------------------------------------------------------
// DOCX
// ---------------------------------------------------------------------------

fn docx_run(r: &Run, force_bold: bool, out: &mut String) {
    let mut rpr = String::new();
    if r.bold || force_bold {
        rpr.push_str("<w:b/>");
    }
    if r.italic {
        rpr.push_str("<w:i/>");
    }
    if r.underline {
        rpr.push_str("<w:u w:val=\"single\"/>");
    }
    if let Some(pt) = r.size_pt.filter(|p| *p > 0.0) {
        let _ = write!(rpr, "<w:sz w:val=\"{}\"/>", (pt * 2.0).round() as u32);
    }
    out.push_str("<w:r>");
    if !rpr.is_empty() {
        let _ = write!(out, "<w:rPr>{rpr}</w:rPr>");
    }
    for (i, piece) in r.text.split('\n').enumerate() {
        if i > 0 {
            out.push_str("<w:br/>");
        }
        for (j, part) in piece.split('\t').enumerate() {
            if j > 0 {
                out.push_str("<w:tab/>");
            }
            if !part.is_empty() {
                let _ = write!(out, "<w:t xml:space=\"preserve\">{}</w:t>", escape(part));
            }
        }
    }
    if r.line_break {
        out.push_str("<w:br/>");
    }
    out.push_str("</w:r>");
}

fn docx_paragraph(p: &Paragraph, force_bold: bool, out: &mut String) {
    let mut ppr = String::new();
    match (p.heading, p.list) {
        (Some(1), _) => ppr.push_str("<w:pStyle w:val=\"Title\"/>"),
        (Some(l), _) => {
            let _ = write!(ppr, "<w:pStyle w:val=\"Heading{}\"/>", l.clamp(2, 4));
        }
        (None, Some(ListKind::Bullet)) => ppr
            .push_str("<w:pStyle w:val=\"ListBullet\"/><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr>"),
        (None, Some(ListKind::Number)) => ppr
            .push_str("<w:pStyle w:val=\"ListNumber\"/><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"2\"/></w:numPr>"),
        (None, None) => {}
    }
    let jc = match p.align {
        Align::Left => None,
        Align::Center => Some("center"),
        Align::Right => Some("right"),
        Align::Justify => Some("both"),
    };
    if let Some(j) = jc {
        let _ = write!(ppr, "<w:jc w:val=\"{j}\"/>");
    }
    out.push_str("<w:p>");
    if !ppr.is_empty() {
        let _ = write!(out, "<w:pPr>{ppr}</w:pPr>");
    }
    for r in &p.runs {
        docx_run(r, force_bold, out);
    }
    out.push_str("</w:p>");
}

fn docx_table(t: &Table, out: &mut String) {
    let cols = col_count(t);
    let shares = col_shares(t, cols);
    let widths: Vec<u32> = shares.iter().map(|s| (s * DOCX_TEXT_TWIPS as f32).round() as u32).collect();
    out.push_str(
        "<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>\
         <w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"333333\"/><w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"333333\"/>\
         <w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"333333\"/><w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"333333\"/>\
         <w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"333333\"/><w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"333333\"/>\
         </w:tblBorders><w:tblLayout w:type=\"fixed\"/></w:tblPr><w:tblGrid>",
    );
    for w in &widths {
        let _ = write!(out, "<w:gridCol w:w=\"{w}\"/>");
    }
    out.push_str("</w:tblGrid>");
    for row in &t.rows {
        let header = !row.is_empty() && row.iter().all(|c| c.header);
        out.push_str("<w:tr>");
        if header {
            out.push_str("<w:trPr><w:tblHeader/></w:trPr>");
        }
        let mut col_i = 0usize;
        for c in row {
            let span = c.col_span.max(1) as usize;
            let w: u32 = widths.iter().skip(col_i).take(span).sum();
            col_i += span;
            out.push_str("<w:tc><w:tcPr>");
            let _ = write!(out, "<w:tcW w:w=\"{w}\" w:type=\"dxa\"/>");
            if span > 1 {
                let _ = write!(out, "<w:gridSpan w:val=\"{span}\"/>");
            }
            if c.header {
                out.push_str("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F2F2F2\"/>");
            }
            out.push_str("</w:tcPr>");
            for p in cell_paragraphs(c) {
                let mut p = p;
                if c.header && p.align == Align::Left {
                    p.align = Align::Center;
                }
                docx_paragraph(&p, c.header, out);
            }
            out.push_str("</w:tc>");
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl>");
}

const DOCX_STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Malgun Gothic" w:hAnsi="Malgun Gothic" w:eastAsia="맑은 고딕" w:cs="Malgun Gothic"/><w:sz w:val="21"/><w:szCs w:val="21"/><w:lang w:val="en-US" w:eastAsia="ko-KR"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="120" w:line="300" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:jc w:val="center"/><w:spacing w:after="360"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="40"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="360" w:after="120"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="28"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="80"/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/><w:sz w:val="24"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading4"><w:name w:val="heading 4"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:outlineLvl w:val="3"/></w:pPr><w:rPr><w:b/><w:sz w:val="22"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="ListBullet"><w:name w:val="List Bullet"/><w:basedOn w:val="Normal"/><w:pPr><w:numPr><w:numId w:val="1"/></w:numPr><w:spacing w:after="40"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="ListNumber"><w:name w:val="List Number"/><w:basedOn w:val="Normal"/><w:pPr><w:numPr><w:numId w:val="2"/></w:numPr><w:spacing w:after="40"/></w:pPr></w:style><w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:tblPr><w:tblCellMar><w:top w:w="40" w:type="dxa"/><w:left w:w="100" w:type="dxa"/><w:bottom w:w="40" w:type="dxa"/><w:right w:w="100" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style><w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:basedOn w:val="TableNormal"/><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="333333"/><w:left w:val="single" w:sz="4" w:space="0" w:color="333333"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="333333"/><w:right w:val="single" w:sz="4" w:space="0" w:color="333333"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="333333"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="333333"/></w:tblBorders></w:tblPr></w:style></w:styles>"#;

const DOCX_NUMBERING: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="singleLevel"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="•"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="480" w:hanging="240"/></w:pPr></w:lvl></w:abstractNum><w:abstractNum w:abstractNumId="1"><w:multiLevelType w:val="singleLevel"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="480" w:hanging="300"/></w:pPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num></w:numbering>"#;

/// Writes `model` as a DOCX file.
pub fn write_docx(model: &DocModel, path: &Path) -> Result<()> {
    let mut body = String::with_capacity(8_192);
    for b in &model.blocks {
        match b {
            Block::Paragraph(p) => docx_paragraph(p, false, &mut body),
            Block::Table(t) => {
                docx_table(t, &mut body);
                // Word needs a paragraph between consecutive tables and after the last one.
                body.push_str("<w:p/>");
            }
            Block::PageBreak => body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>"),
            Block::Image { .. } => {}
        }
    }
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><w:body>{body}\
         <w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>"
    );
    let file = File::create(path).map_err(|e| CoreError::io(path, e))?;
    let mut z = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip_write(&mut z, path, "[Content_Types].xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/></Types>"#, opts)?;
    zip_write(&mut z, path, "_rels/.rels", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/></Relationships>"#, opts)?;
    zip_write(&mut z, path, "docProps/app.xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>DocuLens</Application></Properties>"#, opts)?;
    zip_write(&mut z, path, "word/_rels/document.xml.rels", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/></Relationships>"#, opts)?;
    zip_write(&mut z, path, "word/styles.xml", DOCX_STYLES.as_bytes(), opts)?;
    zip_write(&mut z, path, "word/numbering.xml", DOCX_NUMBERING.as_bytes(), opts)?;
    zip_write(&mut z, path, "word/document.xml", document.as_bytes(), opts)?;
    z.finish().map_err(|e| CoreError::Convert(e.to_string()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// HWPX
// ---------------------------------------------------------------------------

/// Character / paragraph property ids defined in `header.xml` (see [`HWPX_HEADER`]).
const CHAR_NORMAL: u32 = 0;
const CHAR_TITLE: u32 = 1;
const CHAR_HEADING: u32 = 2;
const CHAR_BOLD: u32 = 3;
const CHAR_ITALIC: u32 = 4;
const CHAR_UNDERLINE: u32 = 5;
const CHAR_SMALL: u32 = 6;
const PARA_JUSTIFY: u32 = 0;
const PARA_TITLE: u32 = 1;
const PARA_HEADING2: u32 = 2;
const PARA_BULLET: u32 = 3;
const PARA_NUMBER: u32 = 4;
const PARA_RIGHT: u32 = 5;
const PARA_CENTER: u32 = 6;
const PARA_LEFT: u32 = 7;
const PARA_HEADING3: u32 = 8;

const HWPX_HEADER: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core" version="1.4" secCnt="1"><hh:beginNum page="1" footnote="1" endnote="1" pic="1" tbl="1" equation="1"/><hh:refList><hh:fontfaces itemCnt="7"><hh:fontface lang="HANGUL" fontCnt="1"><hh:font id="0" face="맑은 고딕" type="TTF" isEmbedded="0"/></hh:fontface><hh:fontface lang="LATIN" fontCnt="1"><hh:font id="0" face="맑은 고딕" type="TTF" isEmbedded="0"/></hh:fontface><hh:fontface lang="HANJA" fontCnt="1"><hh:font id="0" face="맑은 고딕" type="TTF" isEmbedded="0"/></hh:fontface><hh:fontface lang="JAPANESE" fontCnt="1"><hh:font id="0" face="맑은 고딕" type="TTF" isEmbedded="0"/></hh:fontface><hh:fontface lang="OTHER" fontCnt="1"><hh:font id="0" face="맑은 고딕" type="TTF" isEmbedded="0"/></hh:fontface><hh:fontface lang="SYMBOL" fontCnt="1"><hh:font id="0" face="맑은 고딕" type="TTF" isEmbedded="0"/></hh:fontface><hh:fontface lang="USER" fontCnt="1"><hh:font id="0" face="맑은 고딕" type="TTF" isEmbedded="0"/></hh:fontface></hh:fontfaces><hh:borderFills itemCnt="3"><hh:borderFill id="1" threeD="0" shadow="0" centerLine="NONE" breakCellSeparateLine="0"><hh:slash type="NONE" Crooked="0" isCounter="0"/><hh:backSlash type="NONE" Crooked="0" isCounter="0"/><hh:leftBorder type="NONE" width="0.1 mm" color="#000000"/><hh:rightBorder type="NONE" width="0.1 mm" color="#000000"/><hh:topBorder type="NONE" width="0.1 mm" color="#000000"/><hh:bottomBorder type="NONE" width="0.1 mm" color="#000000"/><hh:diagonal type="SOLID" width="0.1 mm" color="#000000"/></hh:borderFill><hh:borderFill id="2" threeD="0" shadow="0" centerLine="NONE" breakCellSeparateLine="0"><hh:slash type="NONE" Crooked="0" isCounter="0"/><hh:backSlash type="NONE" Crooked="0" isCounter="0"/><hh:leftBorder type="SOLID" width="0.12 mm" color="#333333"/><hh:rightBorder type="SOLID" width="0.12 mm" color="#333333"/><hh:topBorder type="SOLID" width="0.12 mm" color="#333333"/><hh:bottomBorder type="SOLID" width="0.12 mm" color="#333333"/><hh:diagonal type="SOLID" width="0.1 mm" color="#000000"/></hh:borderFill><hh:borderFill id="3" threeD="0" shadow="0" centerLine="NONE" breakCellSeparateLine="0"><hh:slash type="NONE" Crooked="0" isCounter="0"/><hh:backSlash type="NONE" Crooked="0" isCounter="0"/><hh:leftBorder type="SOLID" width="0.12 mm" color="#333333"/><hh:rightBorder type="SOLID" width="0.12 mm" color="#333333"/><hh:topBorder type="SOLID" width="0.12 mm" color="#333333"/><hh:bottomBorder type="SOLID" width="0.12 mm" color="#333333"/><hh:diagonal type="SOLID" width="0.1 mm" color="#000000"/><hc:fillBrush><hc:winBrush faceColor="#F2F2F2" hatchColor="#999999" alpha="0"/></hc:fillBrush></hh:borderFill></hh:borderFills><hh:charProperties itemCnt="7"><hh:charPr id="0" height="1050" textColor="#000000" shadeColor="none" useFontSpace="0" useKerning="0" symMark="NONE" borderFillIDRef="1"><hh:fontRef hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:ratio hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:spacing hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:relSz hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:offset hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/></hh:charPr><hh:charPr id="1" height="2000" textColor="#000000" shadeColor="none" useFontSpace="0" useKerning="0" symMark="NONE" borderFillIDRef="1"><hh:fontRef hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:ratio hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:spacing hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:relSz hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:offset hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:bold/></hh:charPr><hh:charPr id="2" height="1300" textColor="#000000" shadeColor="none" useFontSpace="0" useKerning="0" symMark="NONE" borderFillIDRef="1"><hh:fontRef hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:ratio hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:spacing hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:relSz hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:offset hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:bold/></hh:charPr><hh:charPr id="3" height="1050" textColor="#000000" shadeColor="none" useFontSpace="0" useKerning="0" symMark="NONE" borderFillIDRef="1"><hh:fontRef hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:ratio hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:spacing hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:relSz hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:offset hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:bold/></hh:charPr><hh:charPr id="4" height="1050" textColor="#000000" shadeColor="none" useFontSpace="0" useKerning="0" symMark="NONE" borderFillIDRef="1"><hh:fontRef hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:ratio hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:spacing hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:relSz hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:offset hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:italic/></hh:charPr><hh:charPr id="5" height="1050" textColor="#000000" shadeColor="none" useFontSpace="0" useKerning="0" symMark="NONE" borderFillIDRef="1"><hh:fontRef hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:ratio hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:spacing hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:relSz hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:offset hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:underline type="BOTTOM" shape="SOLID" color="#000000"/></hh:charPr><hh:charPr id="6" height="900" textColor="#444444" shadeColor="none" useFontSpace="0" useKerning="0" symMark="NONE" borderFillIDRef="1"><hh:fontRef hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:ratio hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:spacing hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:relSz hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:offset hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/></hh:charPr></hh:charProperties><hh:tabProperties itemCnt="1"><hh:tabPr id="0" autoTabLeft="0" autoTabRight="0"/></hh:tabProperties><hh:numberings itemCnt="1"><hh:numbering id="1" start="0"><hh:paraHead start="1" level="1" align="LEFT" useInstWidth="1" autoIndent="1" widthAdjust="0" textOffsetType="PERCENT" textOffset="50" numFormat="DIGIT" charPrIDRef="4294967295" checkable="0">^1.</hh:paraHead></hh:numbering></hh:numberings><hh:bullets itemCnt="1"><hh:bullet id="1" char="•" checkedChar="" useImage="0"><hh:img binaryItemIDRef="" bright="0" contrast="0" effect="REAL_PIC" alpha="0"/><hh:paraHead start="1" level="1" align="LEFT" useInstWidth="1" autoIndent="1" widthAdjust="0" textOffsetType="PERCENT" textOffset="50" numFormat="DIGIT" charPrIDRef="4294967295" checkable="0">^1.</hh:paraHead></hh:bullet></hh:bullets><hh:paraProperties itemCnt="9"><hh:paraPr id="0" tabPrIDRef="0" condense="0" fontLineHeight="0" snapToGrid="1" suppressLineNumbers="0" checked="0"><hh:align horizontal="JUSTIFY" vertical="BASELINE"/><hh:heading type="NONE" idRef="0" level="0"/><hh:breakSetting breakLatinWord="KEEP_WORD" breakNonLatinWord="KEEP_WORD" widowOrphan="0" keepWithNext="0" keepLines="0" pageBreakBefore="0" lineWrap="BREAK"/><hh:margin><hc:intent value="0" unit="HWPUNIT"/><hc:left value="0" unit="HWPUNIT"/><hc:right value="0" unit="HWPUNIT"/><hc:prev value="0" unit="HWPUNIT"/><hc:next value="300" unit="HWPUNIT"/></hh:margin><hh:lineSpacing type="PERCENT" value="160" unit="HWPUNIT"/></hh:paraPr><hh:paraPr id="1" tabPrIDRef="0" condense="0" fontLineHeight="0" snapToGrid="1" suppressLineNumbers="0" checked="0"><hh:align horizontal="CENTER" vertical="BASELINE"/><hh:heading type="OUTLINE" idRef="0" level="0"/><hh:margin><hc:intent value="0" unit="HWPUNIT"/><hc:left value="0" unit="HWPUNIT"/><hc:right value="0" unit="HWPUNIT"/><hc:prev value="0" unit="HWPUNIT"/><hc:next value="1500" unit="HWPUNIT"/></hh:margin><hh:lineSpacing type="PERCENT" value="160" unit="HWPUNIT"/></hh:paraPr><hh:paraPr id="2" tabPrIDRef="0" condense="0" fontLineHeight="0" snapToGrid="1" suppressLineNumbers="0" checked="0"><hh:align horizontal="LEFT" vertical="BASELINE"/><hh:heading type="OUTLINE" idRef="0" level="1"/><hh:margin><hc:intent value="0" unit="HWPUNIT"/><hc:left value="0" unit="HWPUNIT"/><hc:right value="0" unit="HWPUNIT"/><hc:prev value="900" unit="HWPUNIT"/><hc:next value="300" unit="HWPUNIT"/></hh:margin><hh:lineSpacing type="PERCENT" value="160" unit="HWPUNIT"/></hh:paraPr><hh:paraPr id="3" tabPrIDRef="0" condense="0" fontLineHeight="0" snapToGrid="1" suppressLineNumbers="0" checked="0"><hh:align horizontal="LEFT" vertical="BASELINE"/><hh:heading type="BULLET" idRef="1" level="0"/><hh:margin><hc:intent value="-600" unit="HWPUNIT"/><hc:left value="1200" unit="HWPUNIT"/><hc:right value="0" unit="HWPUNIT"/><hc:prev value="0" unit="HWPUNIT"/><hc:next value="100" unit="HWPUNIT"/></hh:margin><hh:lineSpacing type="PERCENT" value="160" unit="HWPUNIT"/></hh:paraPr><hh:paraPr id="4" tabPrIDRef="0" condense="0" fontLineHeight="0" snapToGrid="1" suppressLineNumbers="0" checked="0"><hh:align horizontal="LEFT" vertical="BASELINE"/><hh:heading type="NUMBER" idRef="1" level="0"/><hh:margin><hc:intent value="-600" unit="HWPUNIT"/><hc:left value="1200" unit="HWPUNIT"/><hc:right value="0" unit="HWPUNIT"/><hc:prev value="0" unit="HWPUNIT"/><hc:next value="100" unit="HWPUNIT"/></hh:margin><hh:lineSpacing type="PERCENT" value="160" unit="HWPUNIT"/></hh:paraPr><hh:paraPr id="5" tabPrIDRef="0" condense="0" fontLineHeight="0" snapToGrid="1" suppressLineNumbers="0" checked="0"><hh:align horizontal="RIGHT" vertical="BASELINE"/><hh:heading type="NONE" idRef="0" level="0"/><hh:lineSpacing type="PERCENT" value="160" unit="HWPUNIT"/></hh:paraPr><hh:paraPr id="6" tabPrIDRef="0" condense="0" fontLineHeight="0" snapToGrid="1" suppressLineNumbers="0" checked="0"><hh:align horizontal="CENTER" vertical="BASELINE"/><hh:heading type="NONE" idRef="0" level="0"/><hh:lineSpacing type="PERCENT" value="160" unit="HWPUNIT"/></hh:paraPr><hh:paraPr id="7" tabPrIDRef="0" condense="0" fontLineHeight="0" snapToGrid="1" suppressLineNumbers="0" checked="0"><hh:align horizontal="LEFT" vertical="BASELINE"/><hh:heading type="NONE" idRef="0" level="0"/><hh:lineSpacing type="PERCENT" value="160" unit="HWPUNIT"/></hh:paraPr><hh:paraPr id="8" tabPrIDRef="0" condense="0" fontLineHeight="0" snapToGrid="1" suppressLineNumbers="0" checked="0"><hh:align horizontal="LEFT" vertical="BASELINE"/><hh:heading type="OUTLINE" idRef="0" level="2"/><hh:margin><hc:intent value="0" unit="HWPUNIT"/><hc:left value="0" unit="HWPUNIT"/><hc:right value="0" unit="HWPUNIT"/><hc:prev value="600" unit="HWPUNIT"/><hc:next value="200" unit="HWPUNIT"/></hh:margin><hh:lineSpacing type="PERCENT" value="160" unit="HWPUNIT"/></hh:paraPr></hh:paraProperties><hh:styles itemCnt="3"><hh:style id="0" type="PARA" name="바탕글" engName="Normal" paraPrIDRef="0" charPrIDRef="0" nextStyleIDRef="0" langID="1042" lockForm="0"/><hh:style id="1" type="PARA" name="개요 1" engName="Outline 1" paraPrIDRef="1" charPrIDRef="1" nextStyleIDRef="0" langID="1042" lockForm="0"/><hh:style id="2" type="PARA" name="개요 2" engName="Outline 2" paraPrIDRef="2" charPrIDRef="2" nextStyleIDRef="0" langID="1042" lockForm="0"/></hh:styles></hh:refList><hh:compatibleDocument targetProgram="HWP201X"><hh:layoutCompatibility/></hh:compatibleDocument><hh:docOption><hh:linkinfo path="" pageInherit="0" footnoteInherit="0"/></hh:docOption></hh:head>"##;

struct HwpxIds {
    para: u32,
    obj: u32,
}

impl HwpxIds {
    fn next_para(&mut self) -> u32 {
        self.para += 1;
        self.para
    }
    fn next_obj(&mut self) -> u32 {
        self.obj += 1;
        self.obj
    }
}

fn hwpx_text(r: &Run, char_pr: u32, out: &mut String) {
    let _ = write!(out, "<hp:run charPrIDRef=\"{char_pr}\"><hp:t>");
    for (i, piece) in r.text.split('\n').enumerate() {
        if i > 0 {
            out.push_str("<hp:lineBreak/>");
        }
        for (j, part) in piece.split('\t').enumerate() {
            if j > 0 {
                out.push_str("<hp:tab/>");
            }
            out.push_str(&escape(part));
        }
    }
    if r.line_break {
        out.push_str("<hp:lineBreak/>");
    }
    out.push_str("</hp:t></hp:run>");
}

fn hwpx_char_pr(r: &Run, p: &Paragraph, header_cell: bool) -> u32 {
    match p.heading {
        Some(1) => CHAR_TITLE,
        Some(_) => CHAR_HEADING,
        None => {
            if r.size_pt.is_some_and(|s| s <= 9.5) {
                CHAR_SMALL
            } else if r.bold || header_cell {
                CHAR_BOLD
            } else if r.italic {
                CHAR_ITALIC
            } else if r.underline {
                CHAR_UNDERLINE
            } else {
                CHAR_NORMAL
            }
        }
    }
}

fn hwpx_para_pr(p: &Paragraph) -> (u32, u32) {
    match (p.heading, p.list) {
        (Some(1), _) => (PARA_TITLE, 1),
        (Some(2), _) => (PARA_HEADING2, 2),
        (Some(_), _) => (PARA_HEADING3, 0),
        (None, Some(ListKind::Bullet)) => (PARA_BULLET, 0),
        (None, Some(ListKind::Number)) => (PARA_NUMBER, 0),
        (None, None) => (
            match p.align {
                Align::Left => PARA_LEFT,
                Align::Center => PARA_CENTER,
                Align::Right => PARA_RIGHT,
                Align::Justify => PARA_JUSTIFY,
            },
            0,
        ),
    }
}

fn hwpx_paragraph(p: &Paragraph, ids: &mut HwpxIds, header_cell: bool, extra_runs: &str, out: &mut String) {
    let (para_pr, style) = hwpx_para_pr(p);
    let para_pr = if header_cell && p.heading.is_none() && p.list.is_none() { PARA_CENTER } else { para_pr };
    let _ = write!(
        out,
        "<hp:p id=\"{}\" paraPrIDRef=\"{para_pr}\" styleIDRef=\"{style}\" pageBreak=\"0\" columnBreak=\"0\" merged=\"0\">",
        ids.next_para()
    );
    out.push_str(extra_runs);
    if p.runs.is_empty() && extra_runs.is_empty() {
        let _ = write!(out, "<hp:run charPrIDRef=\"{CHAR_NORMAL}\"><hp:t></hp:t></hp:run>");
    }
    for r in &p.runs {
        hwpx_text(r, hwpx_char_pr(r, p, header_cell), out);
    }
    out.push_str("</hp:p>");
}

fn hwpx_table(t: &Table, ids: &mut HwpxIds, out: &mut String) {
    let cols = col_count(t);
    let shares = col_shares(t, cols);
    let widths: Vec<u32> = shares.iter().map(|s| (s * HWPX_TEXT_UNITS as f32).round() as u32).collect();
    let rows = t.rows.len().max(1);
    let tbl_id = ids.next_obj();
    let mut tbl = String::new();
    let _ = write!(
        tbl,
        "<hp:tbl id=\"{tbl_id}\" zOrder=\"0\" numberingType=\"TABLE\" textWrap=\"TOP_AND_BOTTOM\" textFlow=\"BOTH_SIDES\" lock=\"0\" dropcapstyle=\"None\" pageBreak=\"CELL\" repeatHeader=\"1\" rowCnt=\"{rows}\" colCnt=\"{cols}\" cellSpacing=\"0\" borderFillIDRef=\"2\" noAdjust=\"0\">\
         <hp:sz width=\"{HWPX_TEXT_UNITS}\" height=\"{}\" widthRelTo=\"ABSOLUTE\" heightRelTo=\"ABSOLUTE\" protect=\"0\"/>\
         <hp:pos treatAsChar=\"1\" affectLSpacing=\"0\" flowWithText=\"1\" allowOverlap=\"0\" holdAnchorAndSO=\"0\" vertRelTo=\"PARA\" horzRelTo=\"COLUMN\" vertAlign=\"TOP\" horzAlign=\"LEFT\" vertOffset=\"0\" horzOffset=\"0\"/>\
         <hp:outMargin left=\"283\" right=\"283\" top=\"283\" bottom=\"283\"/>",
        rows as u32 * HWPX_ROW_HEIGHT
    );
    for (ri, row) in t.rows.iter().enumerate() {
        tbl.push_str("<hp:tr>");
        let mut col_i = 0usize;
        for c in row {
            let span = (c.col_span.max(1) as usize).min(cols.saturating_sub(col_i).max(1));
            let w: u32 = widths.iter().skip(col_i).take(span).sum();
            let _ = write!(
                tbl,
                "<hp:tc name=\"\" header=\"{}\" hasMargin=\"0\" protect=\"0\" editable=\"0\" dirty=\"0\" borderFillIDRef=\"{}\">\
                 <hp:subList id=\"\" textDirection=\"HORIZONTAL\" lineWrap=\"BREAK\" vertAlign=\"CENTER\" linkListIDRef=\"0\" linkListNextIDRef=\"0\" textWidth=\"0\" textHeight=\"0\" hasTextRef=\"0\" hasNumRef=\"0\">",
                c.header as u8,
                if c.header { 3 } else { 2 }
            );
            for p in cell_paragraphs(c) {
                hwpx_paragraph(&p, ids, c.header, "", &mut tbl);
            }
            let _ = write!(
                tbl,
                "</hp:subList><hp:cellAddr colAddr=\"{col_i}\" rowAddr=\"{ri}\"/><hp:cellSpan colSpan=\"{span}\" rowSpan=\"1\"/>\
                 <hp:cellSz width=\"{w}\" height=\"{HWPX_ROW_HEIGHT}\"/><hp:cellMargin left=\"510\" right=\"510\" top=\"141\" bottom=\"141\"/></hp:tc>"
            );
            col_i += span;
        }
        tbl.push_str("</hp:tr>");
    }
    tbl.push_str("</hp:tbl>");
    // A table lives inside a run of its own paragraph.
    let run = format!("<hp:run charPrIDRef=\"{CHAR_NORMAL}\">{tbl}</hp:run>");
    hwpx_paragraph(&Paragraph::default(), ids, false, &run, out);
}

/// Writes `model` as an HWPX (OWPML) package.
pub fn write_hwpx(model: &DocModel, path: &Path) -> Result<()> {
    let mut ids = HwpxIds { para: 0, obj: 100 };
    let mut sec = String::with_capacity(8_192);
    sec.push_str(
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core">"##,
    );
    // Section properties ride on the first paragraph (A4 portrait, 30 mm / 20 mm margins).
    let sec_pr = format!(
        "<hp:run charPrIDRef=\"{CHAR_NORMAL}\"><hp:secPr id=\"\" textDirection=\"HORIZONTAL\" spaceColumns=\"1134\" tabStop=\"8000\" tabStopVal=\"4000\" tabStopUnit=\"HWPUNIT\" outlineShapeIDRef=\"1\" memoShapeIDRef=\"0\" textVerticalWidthHead=\"0\" masterPageCnt=\"0\">\
         <hp:grid lineGrid=\"0\" charGrid=\"0\" wonggojiFormat=\"0\"/><hp:startNum pageStartsOn=\"BOTH\" page=\"0\" pic=\"0\" tbl=\"0\" equation=\"0\"/><hp:visibility hideFirstHeader=\"0\" hideFirstFooter=\"0\" hideFirstMasterPage=\"0\" border=\"SHOW_ALL\" fill=\"SHOW_ALL\" hideFirstPageNum=\"0\" hideFirstEmptyLine=\"0\" showLineNumber=\"0\"/>\
         <hp:pagePr landscape=\"WIDELY\" width=\"59528\" height=\"84188\" gutterType=\"LEFT_ONLY\"><hp:margin header=\"4252\" footer=\"4252\" gutter=\"0\" left=\"8504\" right=\"8504\" top=\"5668\" bottom=\"4252\"/></hp:pagePr></hp:secPr></hp:run>"
    );
    let mut first = true;
    for b in &model.blocks {
        match b {
            Block::Paragraph(p) => {
                hwpx_paragraph(p, &mut ids, false, if first { &sec_pr } else { "" }, &mut sec);
                first = false;
            }
            Block::Table(t) => {
                if first {
                    hwpx_paragraph(&Paragraph::default(), &mut ids, false, &sec_pr, &mut sec);
                    first = false;
                }
                hwpx_table(t, &mut ids, &mut sec);
            }
            Block::PageBreak => {
                let _ = write!(
                    sec,
                    "<hp:p id=\"{}\" paraPrIDRef=\"{PARA_LEFT}\" styleIDRef=\"0\" pageBreak=\"1\" columnBreak=\"0\" merged=\"0\"><hp:run charPrIDRef=\"{CHAR_NORMAL}\"><hp:t></hp:t></hp:run></hp:p>",
                    ids.next_para()
                );
            }
            Block::Image { .. } => {}
        }
    }
    if first {
        hwpx_paragraph(&Paragraph::default(), &mut ids, false, &sec_pr, &mut sec);
    }
    sec.push_str("</hs:sec>");

    let file = File::create(path).map_err(|e| CoreError::io(path, e))?;
    let mut z = zip::ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip_write(&mut z, path, "mimetype", b"application/hwp+zip", stored)?;
    zip_write(&mut z, path, "version.xml", br##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><hv:HCFVersion xmlns:hv="http://www.hancom.co.kr/hwpml/2011/version" tagetApplication="WORDPROCESSOR" major="5" minor="1" micro="1" buildNumber="0" os="1" xmlVersion="1.4" application="DocuLens" appVersion="1.0"/>"##, deflated)?;
    zip_write(&mut z, path, "META-INF/container.xml", br##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><ocf:container xmlns:ocf="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:hpf="http://www.hancom.co.kr/schema/2011/hpf"><ocf:rootfiles><ocf:rootfile full-path="Contents/content.hpf" media-type="application/hwpml-package+xml"/></ocf:rootfiles></ocf:container>"##, deflated)?;
    zip_write(&mut z, path, "META-INF/manifest.xml", br##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><odf:manifest xmlns:odf="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><odf:file-entry odf:full-path="/" odf:media-type="application/hwp+zip"/><odf:file-entry odf:full-path="Contents/header.xml" odf:media-type="application/xml"/><odf:file-entry odf:full-path="Contents/section0.xml" odf:media-type="application/xml"/></odf:manifest>"##, deflated)?;
    let title = escape(model.title.as_deref().unwrap_or("document"));
    let hpf = format!(
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><opf:package xmlns:opf="http://www.idpf.org/2007/opf/" version="" unique-identifier="" id=""><opf:metadata><opf:title>{title}</opf:title><opf:language>ko</opf:language><opf:meta name="creator" content="DocuLens"/></opf:metadata><opf:manifest><opf:item id="header" href="Contents/header.xml" media-type="application/xml"/><opf:item id="section0" href="Contents/section0.xml" media-type="application/xml"/></opf:manifest><opf:spine><opf:itemref idref="header" linear="yes"/><opf:itemref idref="section0" linear="yes"/></opf:spine></opf:package>"##
    );
    zip_write(&mut z, path, "Contents/content.hpf", hpf.as_bytes(), deflated)?;
    zip_write(&mut z, path, "Contents/header.xml", HWPX_HEADER.as_bytes(), deflated)?;
    zip_write(&mut z, path, "Contents/section0.xml", sec.as_bytes(), deflated)?;
    z.finish().map_err(|e| CoreError::Convert(e.to_string()))?;
    Ok(())
}
