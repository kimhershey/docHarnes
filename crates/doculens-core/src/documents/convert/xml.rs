//! Minimal DOM on top of quick-xml for the office readers: namespace prefixes
//! are dropped (`hp:p` → `p`), text is unescaped, and depth / size are
//! bounded so a hostile file cannot exhaust memory.

use std::io::Read;

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use crate::error::{CoreError, Result};

/// Largest XML part we are willing to parse (decompressed).
pub const MAX_XML_BYTES: u64 = 256 * 1024 * 1024;
/// Largest binary part (image) we copy out of an archive.
pub const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;
const MAX_DEPTH: usize = 256;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Element {
    /// Local name without namespace prefix.
    pub name: String,
    /// Attribute local names (prefix stripped) and unescaped values.
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Element(Element),
    Text(String),
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn attr_f32(&self, name: &str) -> Option<f32> {
        self.attr(name)?.trim().parse().ok()
    }

    pub fn attr_u32(&self, name: &str) -> Option<u32> {
        self.attr(name)?.trim().parse().ok()
    }

    pub fn attr_bool(&self, name: &str) -> Option<bool> {
        match self.attr(name)?.trim() {
            "1" | "true" | "TRUE" | "on" => Some(true),
            "0" | "false" | "FALSE" | "off" => Some(false),
            _ => None,
        }
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    /// First child element with this local name.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.elements().find(|e| e.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.elements().filter(move |e| e.name == name)
    }

    /// Depth-first search for the first descendant with this name.
    pub fn find(&self, name: &str) -> Option<&Element> {
        for e in self.elements() {
            if e.name == name {
                return Some(e);
            }
            if let Some(f) = e.find(name) {
                return Some(f);
            }
        }
        None
    }

    /// All descendants with this name (document order).
    pub fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a Element>) {
        for e in self.elements() {
            if e.name == name {
                out.push(e);
            }
            e.find_all(name, out);
        }
    }

    /// Concatenated text of all descendant text nodes.
    pub fn text(&self) -> String {
        let mut s = String::new();
        self.collect_text(&mut s);
        s
    }

    fn collect_text(&self, out: &mut String) {
        for n in &self.children {
            match n {
                Node::Text(t) => out.push_str(t),
                Node::Element(e) => e.collect_text(out),
            }
        }
    }
}

fn local_name(raw: &str) -> String {
    match raw.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => raw.to_string(),
    }
}

/// Appends text, merging with a preceding text node (entity references split
/// one logical text run into several events).
fn push_text(parent: &mut Element, text: String) {
    if let Some(Node::Text(last)) = parent.children.last_mut() {
        last.push_str(&text);
    } else {
        parent.children.push(Node::Text(text));
    }
}

/// Parses a whole XML document into an element tree (root element only;
/// comments, processing instructions and the prolog are dropped).
pub fn parse(bytes: &[u8]) -> Result<Element> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = true;
    let mut stack: Vec<Element> = vec![Element { name: "#document".into(), ..Default::default() }];
    let mut buf = Vec::new();
    loop {
        let ev = reader.read_event_into(&mut buf).map_err(|e| CoreError::Convert(format!("xml: {e}")))?;
        match ev {
            Event::Start(start) => {
                if stack.len() > MAX_DEPTH {
                    return Err(CoreError::Convert("xml nesting too deep".into()));
                }
                let mut el = Element { name: local_name(start.local_name().as_ref()), ..Default::default() };
                for a in start.attributes().flatten() {
                    let key = local_name(a.key.as_ref());
                    let value = a.normalized_value(XmlVersion::Implicit1_0).map(|v| v.into_owned()).unwrap_or_default();
                    el.attrs.push((key, value));
                }
                stack.push(el);
            }
            Event::End(_) => {
                let el = stack.pop().ok_or_else(|| CoreError::Convert("xml: unbalanced end tag".into()))?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(Node::Element(el)),
                    None => return Err(CoreError::Convert("xml: unbalanced end tag".into())),
                }
            }
            Event::Text(t) => {
                let text = t.xml_content(XmlVersion::Implicit1_0).into_owned();
                if !text.is_empty() {
                    if let Some(parent) = stack.last_mut() {
                        push_text(parent, text);
                    }
                }
            }
            Event::CData(c) => {
                let text: String = c.as_ref().to_string();
                if let Some(parent) = stack.last_mut() {
                    push_text(parent, text);
                }
            }
            Event::GeneralRef(r) => {
                // `&amp;` / `&#x41;` arrive as separate events; predefined and
                // numeric references are resolved, unknown entities are dropped.
                let resolved: Option<String> = match r.resolve_char_ref() {
                    Ok(Some(c)) => Some(c.to_string()),
                    Ok(None) => match r.as_ref() {
                        "amp" => Some("&".into()),
                        "lt" => Some("<".into()),
                        "gt" => Some(">".into()),
                        "quot" => Some("\"".into()),
                        "apos" => Some("'".into()),
                        _ => None,
                    },
                    Err(_) => None,
                };
                if let (Some(text), Some(parent)) = (resolved, stack.last_mut()) {
                    push_text(parent, text);
                }
            }
            Event::Eof => break,
            Event::Empty(_) => unreachable!("expand_empty_elements is on"),
            Event::Decl(_) | Event::PI(_) | Event::Comment(_) | Event::DocType(_) => {}
        }
        buf.clear();
    }
    let mut doc = stack.pop().ok_or_else(|| CoreError::Convert("xml: empty document".into()))?;
    if !stack.is_empty() {
        return Err(CoreError::Convert("xml: unclosed elements".into()));
    }
    let root = doc.children.drain(..).find_map(|n| match n {
        Node::Element(e) => Some(e),
        Node::Text(_) => None,
    });
    root.ok_or_else(|| CoreError::Convert("xml: no root element".into()))
}

/// Reads one archive entry fully (bounded by `limit`).
pub fn read_zip_entry<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<Option<Vec<u8>>> {
    let index = match archive.index_for_name(name) {
        Some(i) => i,
        None => return Ok(None),
    };
    let mut entry = archive.by_index(index).map_err(|e| CoreError::Convert(format!("zip entry {name}: {e}")))?;
    if entry.size() > limit {
        return Err(CoreError::Convert(format!("zip entry {name} is too large ({} bytes)", entry.size())));
    }
    let mut out = Vec::with_capacity(entry.size() as usize);
    entry
        .by_ref()
        .take(limit)
        .read_to_end(&mut out)
        .map_err(|e| CoreError::Convert(format!("zip entry {name}: {e}")))?;
    Ok(Some(out))
}

/// Entry names of an archive (names that are not valid UTF-8 are skipped).
pub fn zip_names<R: Read + std::io::Seek>(archive: &zip::ZipArchive<R>) -> Vec<String> {
    archive.file_names().filter_map(|n| n.ok().map(|c| c.into_owned())).collect()
}

/// Case-insensitive lookup of an entry name (some producers vary the case of
/// `BinData` / `Contents`).
pub fn find_zip_entry<R: Read + std::io::Seek>(archive: &zip::ZipArchive<R>, name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    zip_names(archive).into_iter().find(|n| n.to_ascii_lowercase() == lower)
}

pub fn mime_for_name(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "tif" | "tiff" => "image/tiff",
        "emf" => "image/emf",
        "wmf" => "image/wmf",
        _ => "application/octet-stream",
    }
}

/// Formats the webview can display inline.
pub fn is_web_image(mime: &str) -> bool {
    matches!(mime, "image/png" | "image/jpeg" | "image/gif" | "image/bmp" | "image/webp" | "image/svg+xml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prefixed_elements_and_text() {
        let xml = br#"<?xml version="1.0"?><hs:sec xmlns:hs="x" xmlns:hp="y"><hp:p id="1" hp:pageBreak="1"><hp:run><hp:t>a &amp; b&#x21;<hp:tab/>c</hp:t></hp:run></hp:p><!-- c --><hp:empty/></hs:sec>"#;
        let root = parse(xml).unwrap();
        assert_eq!(root.name, "sec");
        let p = root.child("p").unwrap();
        assert_eq!(p.attr("id"), Some("1"));
        assert_eq!(p.attr("pageBreak"), Some("1"));
        let t = p.find("t").unwrap();
        assert_eq!(t.text(), "a & b!c");
        assert_eq!(t.elements().count(), 1);
        assert_eq!(t.children.len(), 3, "text runs are merged around entity references");
        assert!(root.child("empty").is_some());
        let mut all = Vec::new();
        root.find_all("t", &mut all);
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn rejects_broken_xml() {
        assert!(parse(b"<a><b></a>").is_err());
        assert!(parse(b"").is_err());
    }
}
