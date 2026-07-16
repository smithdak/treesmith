//! Hand-rolled parser for the layout-XML subset (DESIGN.md §6.1).
//!
//! Supports elements, single/double quoted attributes, self-closing tags,
//! ignorable text/whitespace between elements, and skips the XML
//! declaration and comments. Entities `&lt; &gt; &amp; &quot; &apos;
//! &#N; &#xN;` are decoded in attribute values. No XML dependency.

use std::fmt;

/// One parsed XML element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmlEl {
    /// Tag name (verbatim, including any namespace prefix).
    pub name: String,
    /// Attributes in document order; values are entity-decoded.
    pub attrs: Vec<(String, String)>,
    /// Child elements in document order (text content is ignored).
    pub children: Vec<XmlEl>,
}

impl XmlEl {
    /// The first attribute with this exact name, if present.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A parse failure with the byte offset where it was detected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmlError {
    /// What went wrong.
    pub message: String,
    /// Byte offset into the input at the point of failure.
    pub offset: usize,
}

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at offset {}", self.message, self.offset)
    }
}

impl std::error::Error for XmlError {}

/// Parses one document: optional declaration/comments/whitespace, exactly
/// one root element, optional trailing comments/whitespace.
pub fn parse_xml(s: &str) -> Result<XmlEl, XmlError> {
    let mut p = Parser {
        bytes: s.as_bytes(),
        src: s,
        pos: 0,
    };
    p.skip_misc()?;
    if !p.starts_with(b"<") {
        return Err(p.err("expected root element"));
    }
    let root = p.parse_element()?;
    p.skip_misc()?;
    if p.pos != p.bytes.len() {
        return Err(p.err("unexpected content after root element"));
    }
    Ok(root)
}

struct Parser<'a> {
    bytes: &'a [u8],
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, message: &str) -> XmlError {
        XmlError {
            message: message.to_string(),
            offset: self.pos,
        }
    }

    fn starts_with(&self, prefix: &[u8]) -> bool {
        self.bytes[self.pos..].starts_with(prefix)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while self
            .peek()
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        {
            self.pos += 1;
        }
    }

    /// Skips whitespace, XML declarations / processing instructions, and
    /// comments — the ignorable material between elements.
    fn skip_misc(&mut self) -> Result<(), XmlError> {
        loop {
            self.skip_ws();
            if self.starts_with(b"<?") {
                let start = self.pos;
                match self.src[self.pos..].find("?>") {
                    Some(rel) => self.pos += rel + 2,
                    None => {
                        self.pos = start;
                        return Err(self.err("unterminated XML declaration"));
                    }
                }
            } else if self.starts_with(b"<!--") {
                let start = self.pos;
                match self.src[self.pos + 4..].find("-->") {
                    Some(rel) => self.pos += 4 + rel + 3,
                    None => {
                        self.pos = start;
                        return Err(self.err("unterminated comment"));
                    }
                }
            } else {
                return Ok(());
            }
        }
    }

    fn name(&mut self) -> Result<String, XmlError> {
        let start = self.pos;
        while self.peek().is_some_and(is_name_byte) {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(self.err("expected a name"));
        }
        Ok(self.src[start..self.pos].to_string())
    }

    /// Parses `<name attr=".." ...>` and either the self-closing tail or
    /// children plus the matching close tag.
    fn parse_element(&mut self) -> Result<XmlEl, XmlError> {
        debug_assert!(self.starts_with(b"<"));
        self.pos += 1;
        let name = self.name()?;
        let mut attrs = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'/') => {
                    self.pos += 1;
                    if self.peek() != Some(b'>') {
                        return Err(self.err("expected `>` after `/`"));
                    }
                    self.pos += 1;
                    return Ok(XmlEl {
                        name,
                        attrs,
                        children: Vec::new(),
                    });
                }
                Some(b'>') => {
                    self.pos += 1;
                    let children = self.parse_children(&name)?;
                    return Ok(XmlEl {
                        name,
                        attrs,
                        children,
                    });
                }
                Some(b) if is_name_byte(b) => {
                    let attr_name = self.name()?;
                    self.skip_ws();
                    if self.peek() != Some(b'=') {
                        return Err(self.err("expected `=` after attribute name"));
                    }
                    self.pos += 1;
                    self.skip_ws();
                    let quote = match self.peek() {
                        Some(q @ (b'"' | b'\'')) => q,
                        _ => return Err(self.err("expected quoted attribute value")),
                    };
                    self.pos += 1;
                    let start = self.pos;
                    while self.peek().is_some_and(|b| b != quote) {
                        self.pos += 1;
                    }
                    if self.peek() != Some(quote) {
                        return Err(self.err("unterminated attribute value"));
                    }
                    let raw = &self.src[start..self.pos];
                    let value = decode_entities(raw).map_err(|(msg, rel)| XmlError {
                        message: msg,
                        offset: start + rel,
                    })?;
                    self.pos += 1;
                    attrs.push((attr_name, value));
                }
                _ => return Err(self.err("expected attribute, `/>`, or `>`")),
            }
        }
    }

    /// Children of an open element up to and including `</name>`.
    /// Text runs between elements are ignored.
    fn parse_children(&mut self, open_name: &str) -> Result<Vec<XmlEl>, XmlError> {
        let mut children = Vec::new();
        loop {
            // Ignorable text: everything up to the next `<`.
            while self.peek().is_some_and(|b| b != b'<') {
                self.pos += 1;
            }
            if self.peek().is_none() {
                return Err(self.err(&format!("unexpected end of input inside <{open_name}>")));
            }
            if self.starts_with(b"<!--") || self.starts_with(b"<?") {
                self.skip_misc()?;
                continue;
            }
            if self.starts_with(b"</") {
                self.pos += 2;
                let close = self.name()?;
                if close != open_name {
                    self.pos -= close.len();
                    return Err(self.err(&format!(
                        "mismatched close tag </{close}> for <{open_name}>"
                    )));
                }
                self.skip_ws();
                if self.peek() != Some(b'>') {
                    return Err(self.err("expected `>` in close tag"));
                }
                self.pos += 1;
                return Ok(children);
            }
            children.push(self.parse_element()?);
        }
    }
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':')
}

/// Decodes `&lt; &gt; &amp; &quot; &apos; &#N; &#xN;` in an attribute
/// value. Errors return `(message, byte offset within the raw value)`.
fn decode_entities(raw: &str) -> Result<String, (String, usize)> {
    if !raw.contains('&') {
        return Ok(raw.to_string());
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    let mut base = 0usize;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let ent_start = base + amp;
        let after = &rest[amp + 1..];
        let semi = after
            .find(';')
            .ok_or_else(|| ("unterminated entity".to_string(), ent_start))?;
        let entity = &after[..semi];
        match entity {
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "amp" => out.push('&'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let digits = entity
                    .strip_prefix('#')
                    .ok_or_else(|| (format!("unknown entity `&{entity};`"), ent_start))?;
                let code = if let Some(hex) = digits.strip_prefix('x').or(digits.strip_prefix('X'))
                {
                    u32::from_str_radix(hex, 16)
                } else {
                    digits.parse::<u32>()
                }
                .map_err(|_| (format!("bad character reference `&{entity};`"), ent_start))?;
                let ch = char::from_u32(code)
                    .ok_or_else(|| (format!("bad character reference `&{entity};`"), ent_start))?;
                out.push(ch);
            }
        }
        rest = &after[semi + 1..];
        base = ent_start + 1 + semi + 1;
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_attributes_in_both_quote_styles() {
        let el = parse_xml(r#"<r a="one" b='two'><d id="x"/></r>"#).unwrap();
        assert_eq!(el.name, "r");
        assert_eq!(el.attr("a"), Some("one"));
        assert_eq!(el.attr("b"), Some("two"));
        assert_eq!(el.children.len(), 1);
        assert_eq!(el.children[0].attr("id"), Some("x"));
    }

    #[test]
    fn parses_self_closing_and_nested_elements() {
        let el = parse_xml("<r><d><r uid=\"1\" /><r uid=\"2\"/></d><d/></r>").unwrap();
        assert_eq!(el.children.len(), 2);
        assert_eq!(el.children[0].children.len(), 2);
        assert!(el.children[1].children.is_empty());
    }

    #[test]
    fn skips_declaration_comments_and_text() {
        let src = "<?xml version=\"1.0\"?>\n<!-- top -->\n<r>\n  text ignored\n  <!-- inner -->\n  <d id=\"a\"></d>\n</r>\n";
        let el = parse_xml(src).unwrap();
        assert_eq!(el.children.len(), 1);
        assert_eq!(el.children[0].attr("id"), Some("a"));
    }

    #[test]
    fn decodes_entities_in_attribute_values() {
        let el = parse_xml(r#"<r v="a&lt;b&gt;c&amp;d&quot;e&apos;f&#65;&#x42;"/>"#).unwrap();
        assert_eq!(el.attr("v"), Some("a<b>c&d\"e'fAB"));
    }

    #[test]
    fn namespaced_names_and_attrs_parse() {
        let el =
            parse_xml(r#"<r xmlns:xsd="http://www.w3.org/2001/XMLSchema"><p:x/></r>"#).unwrap();
        assert_eq!(
            el.attr("xmlns:xsd"),
            Some("http://www.w3.org/2001/XMLSchema")
        );
        assert_eq!(el.children[0].name, "p:x");
    }

    #[test]
    fn malformed_inputs_error_with_offsets() {
        // Missing root.
        let e = parse_xml("   ").unwrap_err();
        assert_eq!(e.offset, 3);

        // Unterminated attribute value: offset points at the open quote's text.
        let e = parse_xml(r#"<r a="oops>"#).unwrap_err();
        assert_eq!(e.offset, 11);
        assert!(e.message.contains("unterminated attribute"));

        // Mismatched close tag.
        let e = parse_xml("<r><d></r></d>").unwrap_err();
        assert!(e.message.contains("mismatched close tag"));
        assert_eq!(e.offset, 8, "offset of the close-tag name");

        // Unquoted attribute value.
        let e = parse_xml("<r a=b/>").unwrap_err();
        assert_eq!(e.offset, 5);
        assert!(e.message.contains("quoted attribute value"));

        // Unknown entity.
        let e = parse_xml("<r a=\"x&bogus;\"/>").unwrap_err();
        assert!(e.message.contains("unknown entity"));
        assert_eq!(e.offset, 7, "offset of the `&`");

        // Trailing garbage after root.
        let e = parse_xml("<r/><r/>").unwrap_err();
        assert_eq!(e.offset, 4);

        // EOF inside an element.
        let e = parse_xml("<r><d>").unwrap_err();
        assert_eq!(e.offset, 6);
    }

    #[test]
    fn display_includes_offset() {
        let e = parse_xml("nope").unwrap_err();
        assert!(e.to_string().contains("offset 0"), "{e}");
    }
}
