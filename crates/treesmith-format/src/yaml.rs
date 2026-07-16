//! Fidelity YAML codec for the Rainbow/SCS item subset (DESIGN.md §3.1).
//!
//! Lexical preservation is the I2 strategy: the parser records every
//! presentation detail the emitter needs, so `emit(parse(bytes)) == bytes`
//! holds **by construction** for any file that parses. Files outside the
//! subset are [`ParseFault`]s, never silently skipped (spec §3.4).
//!
//! This is *not* general YAML — it is the Rainbow writer's output grammar.
//! Plain values run verbatim to end of line (so `Value: a: b` is legal),
//! quoting is literal (no escape processing), and block scalars strip a
//! fixed two-space indent per level.

use serde::Serialize;

/// Physical newline style of a document. Mixed styles are a
/// [`FaultKind::MixedNewlines`] parse fault.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Newline {
    /// `\n`
    Lf,
    /// `\r\n`
    CrLf,
}

impl Newline {
    /// The literal newline bytes.
    pub fn as_str(&self) -> &'static str {
        match self {
            Newline::Lf => "\n",
            Newline::CrLf => "\r\n",
        }
    }
}

/// A parsed item document, preserving every lexical detail needed to emit
/// the original bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct YamlDocument {
    /// Leading UTF-8 BOM present in the source.
    pub bom: bool,
    /// Newline style, detected from the first newline.
    pub newline: Newline,
    /// Source ends with a newline.
    pub trailing_newline: bool,
    /// Top-level map entries (after the `---` document marker).
    pub root: Vec<Entry>,
}

/// One `key: value` map entry.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    /// The key text, verbatim (chars up to the first `": "` or trailing `:`).
    pub key: String,
    /// The entry's value.
    pub value: Value,
}

/// An entry's value: a scalar, a nested map, or a list of map items.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Value {
    /// A scalar in one of the four preserved styles.
    Scalar(Scalar),
    /// Nested map at `+2` indent.
    Map(Vec<Entry>),
    /// List of items; each item is a map whose first entry sits inline
    /// after `- ` at the *key's* indent (Rainbow style).
    List(Vec<Vec<Entry>>),
}

/// A scalar value with its exact lexical style.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Scalar {
    /// `K: txt` — txt is everything after `": "`, verbatim (may be `""`).
    Plain(String),
    /// `K:` — no trailing space, empty value.
    PlainBare,
    /// `K: "txt"` — txt is the inner text verbatim, no escape processing.
    Quoted(String),
    /// `K: |` (or `|-`, `>`, ...) — header verbatim; lines stored without
    /// the base indent (blank lines stored as `""`).
    Block {
        /// The header text after `": "`, verbatim (`|`, `|-`, `>`, ...).
        header: String,
        /// Content lines with the base indent stripped.
        lines: Vec<String>,
    },
}

impl Scalar {
    /// The logical value: `Block` joins its lines with `\n`; the other
    /// styles return their text.
    pub fn value(&self) -> String {
        match self {
            Scalar::Plain(t) | Scalar::Quoted(t) => t.clone(),
            Scalar::PlainBare => String::new(),
            Scalar::Block { lines, .. } => lines.join("\n"),
        }
    }
}

/// Why a file could not be parsed (exit-3 class; census failure classes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FaultKind {
    /// The bytes are not valid UTF-8.
    Utf8,
    /// The first line is not exactly `---`.
    MissingDocMarker,
    /// A tab character in indentation.
    TabIndent,
    /// An indent that is not a valid level for its context.
    BadIndent,
    /// A blank line outside a block scalar.
    UnexpectedBlank,
    /// A `- ` list line without an inline `key: value` first entry.
    BadListItem,
    /// A line that is not a recognizable map entry.
    BadStructure,
    /// A CRLF file containing a bare `\n`, or an LF file containing `\r`.
    MixedNewlines,
}

impl FaultKind {
    /// Stable kebab-case name (used in census JSON).
    pub fn as_str(&self) -> &'static str {
        match self {
            FaultKind::Utf8 => "utf8",
            FaultKind::MissingDocMarker => "missing-doc-marker",
            FaultKind::TabIndent => "tab-indent",
            FaultKind::BadIndent => "bad-indent",
            FaultKind::UnexpectedBlank => "unexpected-blank",
            FaultKind::BadListItem => "bad-list-item",
            FaultKind::BadStructure => "bad-structure",
            FaultKind::MixedNewlines => "mixed-newlines",
        }
    }
}

impl Serialize for FaultKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// A parse failure with its location and a human-readable message.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct ParseFault {
    /// What went wrong.
    pub kind: FaultKind,
    /// 1-based line number (line 1 is the `---` marker).
    pub line: usize,
    /// Human-readable detail.
    pub message: String,
}

fn fault(kind: FaultKind, line: usize, message: impl Into<String>) -> ParseFault {
    ParseFault {
        kind,
        line,
        message: message.into(),
    }
}

/// Parses Rainbow-subset YAML bytes into a lossless [`YamlDocument`].
pub fn parse(bytes: &[u8]) -> Result<YamlDocument, ParseFault> {
    let (bom, body) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };
    let text = std::str::from_utf8(body).map_err(|e| {
        let line = body[..e.valid_up_to()]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
            + 1;
        fault(
            FaultKind::Utf8,
            line,
            format!("invalid UTF-8 at byte offset {}", e.valid_up_to()),
        )
    })?;

    // Newline detection from the first `\n`; no newline at all => Lf.
    let newline = match text.find('\n') {
        Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => Newline::CrLf,
        _ => Newline::Lf,
    };
    // Mixed-newline check (DESIGN §3.1 rule 1). A lone `\r` inside a CRLF
    // file is *not* mixed: it stays in line content and round-trips.
    match newline {
        Newline::Lf => {
            if let Some(pos) = text.find('\r') {
                let line = text[..pos].matches('\n').count() + 1;
                return Err(fault(
                    FaultKind::MixedNewlines,
                    line,
                    "carriage return in an LF file",
                ));
            }
        }
        Newline::CrLf => {
            let b = text.as_bytes();
            for (i, &c) in b.iter().enumerate() {
                if c == b'\n' && (i == 0 || b[i - 1] != b'\r') {
                    let line = text[..i].matches('\n').count() + 1;
                    return Err(fault(
                        FaultKind::MixedNewlines,
                        line,
                        "bare newline in a CRLF file",
                    ));
                }
            }
        }
    }

    let nl = newline.as_str();
    let trailing_newline = text.ends_with(nl);
    let mut lines: Vec<&str> = text.split(nl).collect();
    if trailing_newline {
        lines.pop(); // drop the empty segment after the final newline
    }

    if lines.first().copied() != Some("---") {
        return Err(fault(
            FaultKind::MissingDocMarker,
            1,
            "first line must be exactly `---`",
        ));
    }

    let mut parser = Parser { lines, pos: 1 };
    let root = parser.parse_map(0)?;
    debug_assert!(parser.pos >= parser.lines.len());
    Ok(YamlDocument {
        bom,
        newline,
        trailing_newline,
        root,
    })
}

struct Parser<'a> {
    lines: Vec<&'a str>,
    pos: usize,
}

fn leading_spaces(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b' ').count()
}

fn is_blank(s: &str) -> bool {
    s.bytes().all(|b| b == b' ')
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a str> {
        self.lines.get(self.pos).copied()
    }

    /// 1-based line number of the current line.
    fn lineno(&self) -> usize {
        self.pos + 1
    }

    fn advance(&mut self) {
        self.pos += 1;
    }

    /// Parses map entries at exactly `indent`; stops on dedent or EOF.
    fn parse_map(&mut self, indent: usize) -> Result<Vec<Entry>, ParseFault> {
        let mut entries = Vec::new();
        while let Some(line) = self.peek() {
            // Blank (empty or all-space) lines are only legal inside block
            // scalars; the map context is everywhere else.
            if is_blank(line) {
                return Err(fault(
                    FaultKind::UnexpectedBlank,
                    self.lineno(),
                    "blank line outside a block scalar",
                ));
            }
            let ind = leading_spaces(line);
            if ind < indent {
                break; // dedent: caller's context
            }
            if ind > indent {
                return Err(fault(
                    FaultKind::BadIndent,
                    self.lineno(),
                    format!("indent {ind} where {indent} expected"),
                ));
            }
            let content = &line[ind..];
            if content.starts_with('\t') {
                return Err(fault(
                    FaultKind::TabIndent,
                    self.lineno(),
                    "tab character in indentation",
                ));
            }
            if content == "-" || content.starts_with("- ") {
                return Err(fault(
                    FaultKind::BadStructure,
                    self.lineno(),
                    "list item where a map entry was expected",
                ));
            }
            let lineno = self.lineno();
            self.advance();
            entries.push(self.parse_entry(content, indent, lineno)?);
        }
        Ok(entries)
    }

    /// Parses one entry whose `key: ...` text is `content`, at logical
    /// `indent` (for a list's first entry this is the dash indent + 2).
    /// The cursor is already past the entry's own line.
    fn parse_entry(
        &mut self,
        content: &str,
        indent: usize,
        lineno: usize,
    ) -> Result<Entry, ParseFault> {
        let (key, rest): (&str, Option<&str>) = if let Some(i) = content.find(": ") {
            (&content[..i], Some(&content[i + 2..]))
        } else if let Some(k) = content.strip_suffix(':') {
            (k, None)
        } else {
            return Err(fault(
                FaultKind::BadStructure,
                lineno,
                format!("expected `key: value` or `key:`, got `{content}`"),
            ));
        };

        let value = match rest {
            Some(r) if r.len() >= 2 && r.starts_with('"') && r.ends_with('"') => {
                Value::Scalar(Scalar::Quoted(r[1..r.len() - 1].to_string()))
            }
            Some(r) if r.starts_with('|') || r.starts_with('>') => Value::Scalar(Scalar::Block {
                header: r.to_string(),
                lines: self.parse_block_lines(indent + 2)?,
            }),
            // `K: ` — empty value with trailing space. Always Plain("");
            // never a structure header, so the trailing space survives the
            // round trip (a nested structure after `K: ` faults naturally).
            Some(r) => Value::Scalar(Scalar::Plain(r.to_string())),
            // `K:` — look at the next line to decide list / map / bare.
            None => {
                let mut value = None;
                if let Some(next) = self.peek() {
                    if !is_blank(next) {
                        let nind = leading_spaces(next);
                        let ncontent = &next[nind..];
                        if nind == indent && (ncontent == "-" || ncontent.starts_with("- ")) {
                            value = Some(Value::List(self.parse_list(indent)?));
                        } else if nind == indent + 2
                            && !ncontent.starts_with("- ")
                            && ncontent != "-"
                            && (ncontent.contains(": ") || ncontent.ends_with(':'))
                        {
                            value = Some(Value::Map(self.parse_map(indent + 2)?));
                        }
                    }
                }
                value.unwrap_or(Value::Scalar(Scalar::PlainBare))
            }
        };
        Ok(Entry {
            key: key.to_string(),
            value,
        })
    }

    /// Collects block-scalar content lines at `base` indent.
    ///
    /// A line is *blank* only if strictly empty — an all-space line of
    /// length ≥ `base` is a content line (its spaces are content and must
    /// round-trip), except a line of *exactly* `base` spaces, which would
    /// collide with the blank encoding and is therefore a fault.
    fn parse_block_lines(&mut self, base: usize) -> Result<Vec<String>, ParseFault> {
        let mut out = Vec::new();
        let mut pending_blanks = 0usize;
        let mut first_blank_line = 0usize;
        while let Some(line) = self.peek() {
            if line.is_empty() {
                if pending_blanks == 0 {
                    first_blank_line = self.lineno();
                }
                pending_blanks += 1;
                self.advance();
                continue;
            }
            let ind = leading_spaces(line);
            if ind >= base {
                if is_blank(line) && line.len() == base {
                    return Err(fault(
                        FaultKind::UnexpectedBlank,
                        self.lineno(),
                        "whitespace-only line of exactly the block indent",
                    ));
                }
                for _ in 0..pending_blanks {
                    out.push(String::new());
                }
                pending_blanks = 0;
                out.push(line[base..].to_string());
                self.advance();
            } else {
                // Dedent ends the block. A blank line is included only if a
                // later content line follows, or only blanks remain to EOF.
                if pending_blanks > 0 {
                    return Err(fault(
                        FaultKind::UnexpectedBlank,
                        first_blank_line,
                        "blank line between a block scalar and following content",
                    ));
                }
                break;
            }
        }
        // Only blanks remained to EOF: they belong to the block.
        for _ in 0..pending_blanks {
            out.push(String::new());
        }
        Ok(out)
    }

    /// Parses list items whose dashes sit at `indent` (the key's indent).
    fn parse_list(&mut self, indent: usize) -> Result<Vec<Vec<Entry>>, ParseFault> {
        let mut items = Vec::new();
        while let Some(line) = self.peek() {
            if is_blank(line) {
                break; // caller (map context) reports UnexpectedBlank
            }
            let ind = leading_spaces(line);
            if ind != indent {
                break;
            }
            let content = &line[ind..];
            if content == "-" || content == "- " {
                return Err(fault(
                    FaultKind::BadListItem,
                    self.lineno(),
                    "list item without an inline `key: value` first entry",
                ));
            }
            if !content.starts_with("- ") {
                break;
            }
            let first_text = &content[2..];
            let lineno = self.lineno();
            self.advance();
            let first = self
                .parse_entry(first_text, indent + 2, lineno)
                .map_err(|f| {
                    if f.kind == FaultKind::BadStructure && f.line == lineno {
                        fault(
                            FaultKind::BadListItem,
                            lineno,
                            "list item without an inline `key: value` first entry",
                        )
                    } else {
                        f
                    }
                })?;
            let mut item = vec![first];
            item.extend(self.parse_map(indent + 2)?);
            items.push(item);
        }
        Ok(items)
    }
}

/// Emits a document back to bytes — the exact inverse of [`parse`].
pub fn emit(doc: &YamlDocument) -> Vec<u8> {
    let mut lines: Vec<String> = vec!["---".to_string()];
    for entry in &doc.root {
        emit_entry(entry, 0, &mut lines);
    }
    let nl = doc.newline.as_str();
    let mut text = lines.join(nl);
    if doc.trailing_newline {
        text.push_str(nl);
    }
    let mut out = if doc.bom {
        vec![0xEF, 0xBB, 0xBF]
    } else {
        Vec::new()
    };
    out.extend_from_slice(text.as_bytes());
    out
}

fn spaces(n: usize) -> String {
    " ".repeat(n)
}

fn emit_entry(entry: &Entry, indent: usize, out: &mut Vec<String>) {
    let prefix = format!("{}{}", spaces(indent), entry.key);
    match &entry.value {
        Value::Scalar(Scalar::Plain(t)) => out.push(format!("{prefix}: {t}")),
        Value::Scalar(Scalar::PlainBare) => out.push(format!("{prefix}:")),
        Value::Scalar(Scalar::Quoted(t)) => out.push(format!("{prefix}: \"{t}\"")),
        Value::Scalar(Scalar::Block { header, lines }) => {
            out.push(format!("{prefix}: {header}"));
            for l in lines {
                if l.is_empty() {
                    out.push(String::new()); // blank lines carry no padding
                } else {
                    out.push(format!("{}{}", spaces(indent + 2), l));
                }
            }
        }
        Value::Map(entries) => {
            out.push(format!("{prefix}:"));
            for e in entries {
                emit_entry(e, indent + 2, out);
            }
        }
        Value::List(items) => {
            out.push(format!("{prefix}:"));
            for item in items {
                let mut tmp = Vec::new();
                for e in item {
                    emit_entry(e, indent + 2, &mut tmp);
                }
                if let Some(first) = tmp.first_mut() {
                    // Rewrite the first line's `indent + 2` space prefix as
                    // `indent` spaces plus `- ` (the dash sits at the key's
                    // indent, Rainbow style).
                    *first = format!("{}- {}", spaces(indent), &first[indent + 2..]);
                }
                out.extend(tmp);
            }
        }
    }
}

/// Decides the scalar style for *new or changed* values (existing scalars
/// keep their parsed style). Mirrors observed Rainbow writer output
/// (DESIGN.md §3.1 write-rule table; VERIFY-P0 in the P0 census).
pub fn scalar_for_new_value(v: &str) -> Scalar {
    // Rule 1: multiline => block literal (callers reject/normalize `\r`).
    if v.contains('\n') {
        return Scalar::Block {
            header: "|".to_string(),
            lines: v.split('\n').map(str::to_string).collect(),
        };
    }
    // Rule 2: Rainbow never escapes — backslashes and quotes go to a block.
    if v.contains('\\') || v.contains('"') {
        return Scalar::Block {
            header: "|".to_string(),
            lines: vec![v.to_string()],
        };
    }
    // Rule 3: empty => quoted empty (VERIFY-P0).
    if v.is_empty() {
        return Scalar::Quoted(String::new());
    }
    // Rule 4: leading special char / `: ` / trailing space or colon => quoted.
    const SPECIAL_FIRST: &[char] = &[
        '{', '[', '\'', '&', '*', '#', '?', '|', '>', '%', '@', '`', '-', ' ',
    ];
    let first = v.chars().next().expect("non-empty");
    if SPECIAL_FIRST.contains(&first) || v.contains(": ") || v.ends_with(' ') || v.ends_with(':') {
        return Scalar::Quoted(v.to_string());
    }
    // Rule 5: plain.
    Scalar::Plain(v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> YamlDocument {
        parse(src.as_bytes()).expect("should parse")
    }

    fn round_trip(src: &str) {
        let doc = parse_ok(src);
        assert_eq!(
            String::from_utf8(emit(&doc)).unwrap(),
            src,
            "round trip must be byte-identical"
        );
    }

    fn fault_of(src: &[u8]) -> ParseFault {
        parse(src).expect_err("should fault")
    }

    fn scalar_of<'a>(doc: &'a YamlDocument, key: &str) -> &'a Scalar {
        match &doc.root.iter().find(|e| e.key == key).expect("key").value {
            Value::Scalar(s) => s,
            other => panic!("{key} is not a scalar: {other:?}"),
        }
    }

    // ---- physical shell: BOM, newlines, trailing newline ------------------

    #[test]
    fn minimal_document() {
        let doc = parse_ok("---\n");
        assert!(!doc.bom);
        assert_eq!(doc.newline, Newline::Lf);
        assert!(doc.trailing_newline);
        assert!(doc.root.is_empty());
        round_trip("---\n");
    }

    #[test]
    fn no_trailing_newline() {
        let doc = parse_ok("---\nID: x");
        assert!(!doc.trailing_newline);
        round_trip("---\nID: x");
        // even the marker alone, with no newline at all
        let doc = parse_ok("---");
        assert!(!doc.trailing_newline);
        assert_eq!(doc.newline, Newline::Lf);
        round_trip("---");
    }

    #[test]
    fn crlf_document() {
        let src = "---\r\nID: \"x\"\r\nPath: /a\r\n";
        let doc = parse_ok(src);
        assert_eq!(doc.newline, Newline::CrLf);
        assert!(doc.trailing_newline);
        round_trip(src);
    }

    #[test]
    fn bom_document() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"---\nID: x\n");
        let doc = parse(&bytes).unwrap();
        assert!(doc.bom);
        assert_eq!(emit(&doc), bytes);
    }

    #[test]
    fn lone_cr_inside_crlf_line_round_trips() {
        // A stray `\r` not followed by `\n` in a CRLF file is line content.
        let src = "---\r\nValue: a\rb\r\n";
        let doc = parse_ok(src);
        assert_eq!(scalar_of(&doc, "Value").value(), "a\rb");
        round_trip(src);
    }

    // ---- fault kinds -------------------------------------------------------

    #[test]
    fn fault_utf8() {
        let f = fault_of(b"---\nID: \xFF\xFE\n");
        assert_eq!(f.kind, FaultKind::Utf8);
        assert_eq!(f.line, 2);
    }

    #[test]
    fn fault_missing_doc_marker() {
        for src in ["", "ID: x\n", "--- \n", "----\n", " ---\n"] {
            let f = fault_of(src.as_bytes());
            assert_eq!(f.kind, FaultKind::MissingDocMarker, "src={src:?}");
            assert_eq!(f.line, 1);
        }
    }

    #[test]
    fn fault_mixed_newlines_lf_with_cr() {
        let f = fault_of(b"---\nValue: a\rb\n");
        assert_eq!(f.kind, FaultKind::MixedNewlines);
        assert_eq!(f.line, 2);
    }

    #[test]
    fn fault_mixed_newlines_crlf_with_bare_lf() {
        let f = fault_of(b"---\r\nID: x\nPath: /a\r\n");
        assert_eq!(f.kind, FaultKind::MixedNewlines);
        assert_eq!(f.line, 2);
    }

    #[test]
    fn fault_tab_indent() {
        let f = fault_of(b"---\nA:\n\tB: 1\n");
        assert_eq!(f.kind, FaultKind::TabIndent);
        assert_eq!(f.line, 3);
        // tab after correct-level spaces is also indentation
        let f = fault_of(b"---\nA:\n  \tB: 1\n");
        assert_eq!(f.kind, FaultKind::TabIndent);
    }

    #[test]
    fn fault_bad_indent() {
        // over-indented map entry
        let f = fault_of(b"---\nID: x\n  Path: /a\n");
        assert_eq!(f.kind, FaultKind::BadIndent);
        assert_eq!(f.line, 3);
        // odd (non-2-step) indent under a nested map
        let f = fault_of(b"---\nA:\n  B: 1\n C: 2\n");
        assert_eq!(f.kind, FaultKind::BadIndent);
        assert_eq!(f.line, 4);
    }

    #[test]
    fn fault_unexpected_blank_in_map() {
        let f = fault_of(b"---\nID: x\n\nPath: /a\n");
        assert_eq!(f.kind, FaultKind::UnexpectedBlank);
        assert_eq!(f.line, 3);
        // whitespace-only line counts as blank
        let f = fault_of(b"---\nID: x\n   \nPath: /a\n");
        assert_eq!(f.kind, FaultKind::UnexpectedBlank);
    }

    #[test]
    fn fault_unexpected_blank_after_block() {
        // blank between block content and a following dedented line
        let f = fault_of(b"---\nValue: |\n  a\n\nID: x\n");
        assert_eq!(f.kind, FaultKind::UnexpectedBlank);
        assert_eq!(f.line, 4);
    }

    #[test]
    fn fault_blank_line_of_exact_block_indent() {
        // A line of exactly the block indent's spaces cannot be preserved
        // (it would collide with the blank-line encoding), so it faults.
        let f = fault_of(b"---\nValue: |\n  a\n  \n  b\n");
        assert_eq!(f.kind, FaultKind::UnexpectedBlank);
        assert_eq!(f.line, 4);
    }

    #[test]
    fn fault_bad_list_item() {
        // dash with nothing inline
        let f = fault_of(b"---\nSharedFields:\n-\n");
        assert_eq!(f.kind, FaultKind::BadListItem);
        assert_eq!(f.line, 3);
        let f = fault_of(b"---\nSharedFields:\n- \n");
        assert_eq!(f.kind, FaultKind::BadListItem);
        // dash with non-entry text
        let f = fault_of(b"---\nSharedFields:\n- justtext\n");
        assert_eq!(f.kind, FaultKind::BadListItem);
        assert_eq!(f.line, 3);
    }

    #[test]
    fn fault_bad_structure() {
        // no colon at all
        let f = fault_of(b"---\njust some text\n");
        assert_eq!(f.kind, FaultKind::BadStructure);
        assert_eq!(f.line, 2);
        // dash where a map entry was expected (no list-valued key before it)
        let f = fault_of(b"---\nID: x\n- A: 1\n");
        assert_eq!(f.kind, FaultKind::BadStructure);
        assert_eq!(f.line, 3);
    }

    // ---- scalar styles ------------------------------------------------------

    #[test]
    fn plain_scalar_verbatim_to_eol() {
        let src = "---\nValue: a: b: c\n";
        let doc = parse_ok(src);
        assert_eq!(scalar_of(&doc, "Value"), &Scalar::Plain("a: b: c".into()));
        round_trip(src);
    }

    #[test]
    fn plain_scalar_keeps_trailing_spaces() {
        let src = "---\nValue: text   \n";
        let doc = parse_ok(src);
        assert_eq!(scalar_of(&doc, "Value"), &Scalar::Plain("text   ".into()));
        round_trip(src);
    }

    #[test]
    fn plain_empty_vs_bare() {
        // `K: ` (trailing space) is Plain(""); `K:` is PlainBare.
        let src = "---\nEmptyish: \nBare:\n";
        let doc = parse_ok(src);
        assert_eq!(scalar_of(&doc, "Emptyish"), &Scalar::Plain(String::new()));
        assert_eq!(scalar_of(&doc, "Bare"), &Scalar::PlainBare);
        assert_eq!(scalar_of(&doc, "Bare").value(), "");
        round_trip(src);
    }

    #[test]
    fn quoted_scalar_no_escape_processing() {
        let src = "---\nID: \"abc-def\"\nOther: \"a \\ b\"\n";
        let doc = parse_ok(src);
        assert_eq!(scalar_of(&doc, "ID"), &Scalar::Quoted("abc-def".into()));
        assert_eq!(scalar_of(&doc, "Other"), &Scalar::Quoted("a \\ b".into()));
        round_trip(src);
    }

    #[test]
    fn quoted_empty() {
        let src = "---\nValue: \"\"\n";
        let doc = parse_ok(src);
        assert_eq!(scalar_of(&doc, "Value"), &Scalar::Quoted(String::new()));
        round_trip(src);
    }

    #[test]
    fn lone_double_quote_is_plain() {
        // len < 2 cannot be a quoted form
        let src = "---\nValue: \"\n";
        let doc = parse_ok(src);
        assert_eq!(scalar_of(&doc, "Value"), &Scalar::Plain("\"".into()));
        round_trip(src);
        // unterminated quote is plain too
        let src = "---\nValue: \"abc\n";
        let doc = parse_ok(src);
        assert_eq!(scalar_of(&doc, "Value"), &Scalar::Plain("\"abc".into()));
        round_trip(src);
    }

    #[test]
    fn block_scalar_basic() {
        let src = "---\nValue: |\n  line one\n  line two\n";
        let doc = parse_ok(src);
        let s = scalar_of(&doc, "Value");
        assert_eq!(
            s,
            &Scalar::Block {
                header: "|".into(),
                lines: vec!["line one".into(), "line two".into()],
            }
        );
        assert_eq!(s.value(), "line one\nline two");
        round_trip(src);
    }

    #[test]
    fn block_headers_verbatim() {
        for header in ["|", "|-", "|+", ">", ">-", "|2"] {
            let src = format!("---\nValue: {header}\n  x\n");
            let doc = parse_ok(&src);
            match scalar_of(&doc, "Value") {
                Scalar::Block { header: h, .. } => assert_eq!(h, header),
                other => panic!("expected block, got {other:?}"),
            }
            round_trip(&src);
        }
    }

    #[test]
    fn block_with_interior_blank_line() {
        let src = "---\nValue: |\n  a\n\n  b\nID: x\n";
        let doc = parse_ok(src);
        assert_eq!(scalar_of(&doc, "Value").value(), "a\n\nb");
        round_trip(src);
    }

    #[test]
    fn block_with_trailing_blanks_to_eof() {
        let src = "---\nValue: |\n  a\n\n\n";
        let doc = parse_ok(src);
        assert_eq!(scalar_of(&doc, "Value").value(), "a\n\n");
        round_trip(src);
    }

    #[test]
    fn block_deeper_indent_is_content() {
        let src = "---\nValue: |\n  <r>\n    <d id=\"x\" />\n  </r>\n";
        let doc = parse_ok(src);
        assert_eq!(
            scalar_of(&doc, "Value").value(),
            "<r>\n  <d id=\"x\" />\n</r>"
        );
        round_trip(src);
    }

    #[test]
    fn block_all_space_content_line_longer_than_indent() {
        // 4 spaces where base indent is 2: content is 2 spaces, preserved.
        let src = "---\nValue: |\n  a\n    \n  b\n";
        let doc = parse_ok(src);
        assert_eq!(scalar_of(&doc, "Value").value(), "a\n  \nb");
        round_trip(src);
    }

    #[test]
    fn block_with_zero_content_lines() {
        let src = "---\nValue: |\nID: x\n";
        let doc = parse_ok(src);
        assert_eq!(
            scalar_of(&doc, "Value"),
            &Scalar::Block {
                header: "|".into(),
                lines: vec![],
            }
        );
        round_trip(src);
    }

    #[test]
    fn block_backslash_content() {
        let src = "---\nValue: |\n  sitecore\\admin\n";
        let doc = parse_ok(src);
        assert_eq!(scalar_of(&doc, "Value").value(), "sitecore\\admin");
        round_trip(src);
    }

    // ---- maps and lists -----------------------------------------------------

    #[test]
    fn nested_map() {
        let src = "---\nOuter:\n  A: 1\n  B: 2\nAfter: x\n";
        let doc = parse_ok(src);
        match &doc.root[0].value {
            Value::Map(m) => {
                assert_eq!(m.len(), 2);
                assert_eq!(m[0].key, "A");
            }
            other => panic!("expected map, got {other:?}"),
        }
        assert_eq!(doc.root[1].key, "After");
        round_trip(src);
    }

    #[test]
    fn list_dash_at_key_indent() {
        let src =
            "---\nSharedFields:\n- ID: \"a\"\n  Value: one\n- ID: \"b\"\n  Value: two\nPath: /x\n";
        let doc = parse_ok(src);
        match &doc.root[0].value {
            Value::List(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0][0].key, "ID");
                assert_eq!(items[0][1].key, "Value");
            }
            other => panic!("expected list, got {other:?}"),
        }
        assert_eq!(doc.root[1].key, "Path");
        round_trip(src);
    }

    #[test]
    fn realistic_item_round_trip() {
        let src = "---\nID: \"c0ffee00-0001-4000-8000-000000000001\"\nParent: \"aaaaaaaa-0000-4000-8000-0000000000aa\"\nTemplate: \"7c1e1c2a-0020-4000-8000-000000000020\"\nPath: /sitecore/content/Home\nDB: master\nSharedFields:\n- ID: \"7c1e1c2a-0006-4000-8000-000000000006\"\n  Hint: RelatedPages\n  Type: Treelist\n  Value: |\n    {C0FFEE00-0003-4000-8000-000000000003}\nLanguages:\n- Language: da\n  Fields:\n  - ID: \"7c1e1c2a-0005-4000-8000-000000000005\"\n    Hint: NavTitle\n    Value: Hjem\n  Versions:\n  - Version: 1\n    Fields:\n    - ID: \"7c1e1c2a-0003-4000-8000-000000000003\"\n      Hint: Title\n      Value: Hjem\n- Language: en\n  Versions:\n  - Version: 1\n    Fields:\n    - ID: \"7c1e1c2a-0003-4000-8000-000000000003\"\n      Hint: Title\n      Value: Home\n  - Version: 2\n    Fields:\n    - ID: \"7c1e1c2a-0003-4000-8000-000000000003\"\n      Hint: Title\n      Value: Home v2\n    - ID: \"7c1e1c2a-0004-4000-8000-000000000004\"\n      Hint: Body\n      Value: |\n        <p>hello</p>\n\n        <p>world</p>\n";
        round_trip(src);
    }

    #[test]
    fn unknown_keys_preserved_in_order() {
        let src = "---\nID: \"x\"\nMystery: value\nAnotherThing:\n  Deep: 1\nPath: /a\n";
        let doc = parse_ok(src);
        let keys: Vec<&str> = doc.root.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["ID", "Mystery", "AnotherThing", "Path"]);
        round_trip(src);
    }

    #[test]
    fn nested_list_inside_list_item() {
        // Versions structure: list inside a list item's map.
        let src = "---\nLanguages:\n- Language: en\n  Versions:\n  - Version: 1\n    Fields:\n    - ID: \"a\"\n      Value: v\n";
        round_trip(src);
    }

    #[test]
    fn list_first_entry_may_be_block() {
        let src = "---\nItems:\n- Value: |\n    deep\n  Next: 1\n";
        let doc = parse_ok(src);
        match &doc.root[0].value {
            Value::List(items) => {
                assert_eq!(items[0][0].key, "Value");
                assert_eq!(
                    match &items[0][0].value {
                        Value::Scalar(s) => s.value(),
                        _ => panic!(),
                    },
                    "deep"
                );
                assert_eq!(items[0][1].key, "Next");
            }
            other => panic!("expected list, got {other:?}"),
        }
        round_trip(src);
    }

    // ---- scalar_for_new_value write-rule table ------------------------------

    #[test]
    fn write_rule_1_multiline_block() {
        assert_eq!(
            scalar_for_new_value("a\nb"),
            Scalar::Block {
                header: "|".into(),
                lines: vec!["a".into(), "b".into()],
            }
        );
    }

    #[test]
    fn write_rule_2_backslash_or_quote_block() {
        assert_eq!(
            scalar_for_new_value("sitecore\\admin"),
            Scalar::Block {
                header: "|".into(),
                lines: vec!["sitecore\\admin".into()],
            }
        );
        assert_eq!(
            scalar_for_new_value("say \"hi\""),
            Scalar::Block {
                header: "|".into(),
                lines: vec!["say \"hi\"".into()],
            }
        );
    }

    #[test]
    fn write_rule_3_empty_quoted() {
        assert_eq!(scalar_for_new_value(""), Scalar::Quoted(String::new()));
    }

    #[test]
    fn write_rule_4_special_leads_and_colon_space() {
        for v in [
            "{C0FFEE00-0001-4000-8000-000000000001}",
            "[x]",
            "'q",
            "&a",
            "*a",
            "#c",
            "?m",
            "|p",
            ">g",
            "%e",
            "@a",
            "`t",
            "-1",
            " lead",
            "a: b",
            "trail ",
            "end:",
        ] {
            assert_eq!(
                scalar_for_new_value(v),
                Scalar::Quoted(v.to_string()),
                "value {v:?} should be quoted"
            );
        }
    }

    #[test]
    fn write_rule_5_plain() {
        for v in [
            "Home",
            "1",
            "/sitecore/content/Home",
            "a:b",
            "20260101T000000Z",
        ] {
            assert_eq!(
                scalar_for_new_value(v),
                Scalar::Plain(v.to_string()),
                "value {v:?} should be plain"
            );
        }
    }

    #[test]
    fn write_rules_round_trip_through_codec() {
        // Every style the write rules produce must survive emit -> parse.
        for v in [
            "Home",
            "",
            "{C0FFEE00-0001-4000-8000-000000000001}",
            "a: b",
            "multi\nline",
            "back\\slash",
        ] {
            let doc = YamlDocument {
                bom: false,
                newline: Newline::Lf,
                trailing_newline: true,
                root: vec![Entry {
                    key: "Value".into(),
                    value: Value::Scalar(scalar_for_new_value(v)),
                }],
            };
            let bytes = emit(&doc);
            let back = parse(&bytes).unwrap_or_else(|f| panic!("value {v:?}: {f}"));
            assert_eq!(back, doc, "value {v:?} model round trip");
            assert_eq!(
                match &back.root[0].value {
                    Value::Scalar(s) => s.value(),
                    _ => panic!(),
                },
                v,
                "value {v:?} logical round trip"
            );
        }
    }

    // ---- determinism ---------------------------------------------------------

    #[test]
    fn parse_emit_is_deterministic() {
        let src = "---\nID: \"x\"\nSharedFields:\n- ID: \"a\"\n  Value: |\n    v\n";
        let a = emit(&parse_ok(src));
        let b = emit(&parse_ok(src));
        assert_eq!(a, b);
    }
}
